// ---------------------------------------------------------------------------
// État global de l'interface (zustand). Ce qui doit survivre au redémarrage
// (thème, cluster, namespace par cluster, panneau de l'assistant) est
// persisté dans localStorage ; le reste vit le temps de la session.
// ---------------------------------------------------------------------------
import { create } from "zustand";
import { persist } from "zustand/middleware";
import type { ObjectSummary } from "@/api/types";

export type View =
  | "overview"
  | "resources"
  | "topology"
  | "hub"
  | "deploy"
  | "updates"
  | "clusters"
  | "settings";

export const VIEW_LABELS: Record<View, string> = {
  overview: "Vue d'ensemble",
  resources: "Ressources",
  topology: "Topologie",
  hub: "Hub",
  deploy: "Déployer",
  updates: "Mises à jour",
  clusters: "Clusters",
  settings: "Réglages",
};

export type ToastKind = "ok" | "err" | "info" | "warn";

export interface Toast {
  id: number;
  kind: ToastKind;
  title?: string;
  message: string;
}

export type Theme = "auto" | "light" | "dark";

export interface ResourceTarget {
  kind: string;
  name: string;
  namespace: string | null;
}

interface State {
  view: View;
  cluster: string | null;
  namespaceByCluster: Record<string, string | null>;
  theme: Theme;
  autoRefresh: boolean;
  assistantOpen: boolean;
  assistantWidth: number;
  filter: string;
  searchTick: number;
  resourcesKind: string;
  selection: ObjectSummary | null;
  selectionYaml: string | null;
  yamlConsole: { open: boolean; draft: string };
  toasts: Toast[];
  pendingTarget: ResourceTarget | null;
}

interface Actions {
  setView: (v: View) => void;
  setCluster: (name: string | null) => void;
  namespace: () => string | null;
  setNamespace: (ns: string | null) => void;
  setTheme: (t: Theme) => void;
  toggleAutoRefresh: () => void;
  setAssistantOpen: (open: boolean) => void;
  toggleAssistant: () => void;
  setAssistantWidth: (w: number) => void;
  setFilter: (f: string) => void;
  focusSearch: () => void;
  setResourcesKind: (k: string) => void;
  select: (obj: ObjectSummary | null) => void;
  setSelectionYaml: (yaml: string | null) => void;
  openYamlConsole: (draft?: string) => void;
  closeYamlConsole: () => void;
  setYamlDraft: (draft: string) => void;
  toast: (kind: ToastKind, message: string, title?: string) => void;
  dismissToast: (id: number) => void;
  goToResource: (target: ResourceTarget) => void;
  takePendingTarget: () => ResourceTarget | null;
}

let toastSeq = 1;

export const useStore = create<State & Actions>()(
  persist(
    (set, get) => ({
      view: "overview",
      cluster: null,
      namespaceByCluster: {},
      theme: "auto",
      autoRefresh: true,
      assistantOpen: false,
      assistantWidth: 420,
      filter: "",
      searchTick: 0,
      resourcesKind: "pods",
      selection: null,
      selectionYaml: null,
      yamlConsole: { open: false, draft: "" },
      toasts: [],
      pendingTarget: null,

      setView: (view) => set({ view, filter: "" }),
      setCluster: (cluster) => set({ cluster, selection: null, selectionYaml: null }),
      namespace: () => {
        const { cluster, namespaceByCluster } = get();
        if (!cluster) return null;
        return namespaceByCluster[cluster] ?? null;
      },
      setNamespace: (ns) => {
        const { cluster, namespaceByCluster } = get();
        if (!cluster) return;
        set({
          namespaceByCluster: { ...namespaceByCluster, [cluster]: ns },
          selection: null,
          selectionYaml: null,
        });
      },
      setTheme: (theme) => set({ theme }),
      toggleAutoRefresh: () => set((s) => ({ autoRefresh: !s.autoRefresh })),
      setAssistantOpen: (assistantOpen) => set({ assistantOpen }),
      toggleAssistant: () => set((s) => ({ assistantOpen: !s.assistantOpen })),
      setAssistantWidth: (w) => set({ assistantWidth: Math.max(320, Math.min(900, Math.round(w))) }),
      setFilter: (filter) => set({ filter }),
      focusSearch: () => set((s) => ({ searchTick: s.searchTick + 1 })),
      setResourcesKind: (resourcesKind) => set({ resourcesKind, selection: null, selectionYaml: null }),
      select: (selection) => set({ selection, selectionYaml: null }),
      setSelectionYaml: (selectionYaml) => set({ selectionYaml }),
      openYamlConsole: (draft) =>
        set((s) => ({
          view: "resources",
          yamlConsole: { open: true, draft: draft ?? s.yamlConsole.draft },
        })),
      closeYamlConsole: () => set((s) => ({ yamlConsole: { ...s.yamlConsole, open: false } })),
      setYamlDraft: (draft) => set((s) => ({ yamlConsole: { ...s.yamlConsole, draft } })),
      toast: (kind, message, title) => {
        const id = toastSeq++;
        set((s) => ({ toasts: [...s.toasts.slice(-5), { id, kind, message, title }] }));
        const ttl = kind === "err" ? 10_000 : 5_000;
        window.setTimeout(() => get().dismissToast(id), ttl);
      },
      dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
      goToResource: (target) =>
        set({
          view: "resources",
          resourcesKind: target.kind,
          pendingTarget: target,
          filter: "",
        }),
      takePendingTarget: () => {
        const t = get().pendingTarget;
        if (t) set({ pendingTarget: null });
        return t;
      },
    }),
    {
      name: "kubewatch-ui",
      partialize: (s) => ({
        theme: s.theme,
        cluster: s.cluster,
        namespaceByCluster: s.namespaceByCluster,
        autoRefresh: s.autoRefresh,
        assistantOpen: s.assistantOpen,
        assistantWidth: s.assistantWidth,
        resourcesKind: s.resourcesKind,
        view: s.view,
      }),
    },
  ),
);

/** Namespace courant, réactif. */
export function useNamespace(): string | null {
  return useStore((s) => (s.cluster ? (s.namespaceByCluster[s.cluster] ?? null) : null));
}

/** Raccourci : notification depuis du code non-React. */
export const notify = {
  ok: (m: string, t?: string) => useStore.getState().toast("ok", m, t),
  err: (m: string, t?: string) => useStore.getState().toast("err", m, t),
  info: (m: string, t?: string) => useStore.getState().toast("info", m, t),
  warn: (m: string, t?: string) => useStore.getState().toast("warn", m, t),
};
