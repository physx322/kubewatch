// ---------------------------------------------------------------------------
// Accès au backend Rust : une fonction typée par commande Tauri.
// Les erreurs arrivent sous forme de chaîne française prête à afficher ; elles
// sont converties en `Error` pour que React Query et les `catch` les traitent
// uniformément.
// ---------------------------------------------------------------------------
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type * as T from "./types";
import type * as H from "./hub";
import type * as U from "./updates";

export async function call<R>(cmd: string, args?: Record<string, unknown>): Promise<R> {
  try {
    return await invoke<R>(cmd, args);
  } catch (e) {
    throw new Error(describeError(e));
  }
}

export function describeError(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}

// ---------------------------------------------------------------------------
// Normalisation : le cœur Rust omet du JSON les champs `Option` à `None` et
// certains tableaux vides (`skip_serializing_if`). Les types de types.ts les
// déclarent toujours présents ; on comble ici, en un seul endroit, pour que
// `o.owners.length` ou `o.namespace === null` restent vrais partout.
// ---------------------------------------------------------------------------
type Defaults<T> = { [K in keyof T]?: T[K] };

function fill<T extends object>(value: T, defaults: Defaults<T>): T {
  const out = value as Record<string, unknown>;
  for (const [k, v] of Object.entries(defaults)) {
    if (out[k] === undefined) out[k] = v;
  }
  return value;
}

export function normalizeRef(r: T.ResourceRef): T.ResourceRef {
  return fill(r, { group: "", version: "", plural: "", namespace: null });
}

export function normalizeSummary(o: T.ObjectSummary): T.ObjectSummary {
  fill(o, {
    namespace: null,
    uid: null,
    createdAt: null,
    ageSeconds: null,
    status: "",
    ready: null,
    restarts: null,
    node: null,
    images: [],
    labels: {},
    annotations: {},
    owners: [],
    extra: {},
  });
  for (const w of o.owners) fill(w, { uid: null, controller: false });
  return o;
}

export function normalizeCluster(c: T.ClusterInfo): T.ClusterInfo {
  return fill(c, { context: null, version: null, platform: null, nodeCount: null, namespaceCount: null, lastError: null, metricsAvailable: false });
}

function normalizeContext(c: T.ContextInfo): T.ContextInfo {
  return fill(c, { user: null, namespace: null, server: null, current: false });
}

function normalizeEvent(e: T.EventSummary): T.EventSummary {
  return fill(e, { namespace: null, firstSeen: null, lastSeen: null, involvedKind: null, involvedName: null, source: null, count: 0, type: "" });
}

function normalizeMetric(m: T.MetricsSample): T.MetricsSample {
  return fill(m, { namespace: null, container: null, timestamp: null });
}

function normalizeKind(k: T.ResourceKind): T.ResourceKind {
  return fill(k, { group: "", verbs: [], shortNames: [], categories: [] });
}

function normalizeOutcome(o: T.ApplyOutcome): T.ApplyOutcome {
  for (const it of o.items) {
    fill(it, { message: null });
    normalizeRef(it.resource);
  }
  return o;
}

function channel<E>(onEvent: (ev: E) => void): Channel<E> {
  const ch = new Channel<E>();
  ch.onmessage = onEvent;
  return ch;
}

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

export function base64ToBytes(b64: string): Uint8Array {
  const binary = atob(b64);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}

export const api = {
  appInfo: () => call<T.AppInfo>("app_info"),
  openUrl: (url: string) => call<void>("open_url", { url }),
  restart: () => call<void>("app_restart"),

  render: {
    get: () => call<T.RenderView>("render_settings"),
    setAcceleration: (acceleration: T.Acceleration) =>
      call<T.RenderView>("render_set_acceleration", { acceleration }),
    setText: (text: T.TextRendering) => call<T.RenderView>("render_set_text", { text }),
  },

  clusters: {
    list: () => call<T.ClusterInfo[]>("list_clusters").then((l) => l.map(normalizeCluster)),
    connect: (name: string, spec: T.ConnectionSpec, persist: boolean) =>
      call<T.ClusterInfo>("connect_cluster", { name, spec, persist }).then(normalizeCluster),
    importKubeconfig: (path: string | null, allContexts: boolean) =>
      call<string[]>("import_kubeconfig", { path, allContexts }),
    listContexts: (path: string | null) => call<T.ContextInfo[]>("list_contexts", { path }).then((l) => l.map(normalizeContext)),
    defaultKubeconfigPath: () => call<string | null>("default_kubeconfig_path"),
    remove: (name: string) => call<void>("remove_cluster", { name }),
    select: (name: string) => call<void>("select_cluster", { name }),
    current: () => call<string | null>("current_cluster"),
    refreshCatalog: (name: string) => call<void>("refresh_cluster_catalog", { name }),
    pickFile: (title?: string) => call<string | null>("pick_file", { title: title ?? null }),
  },

  resources: {
    overview: (cluster: string) => call<T.ClusterOverview>("cluster_overview", { cluster }),
    kinds: (cluster: string) => call<T.ResourceKind[]>("list_kinds", { cluster }).then((l) => l.map(normalizeKind)),
    namespaces: (cluster: string) => call<string[]>("list_namespaces", { cluster }),
    list: (cluster: string, kind: string, opts: T.ListOptions) =>
      call<T.ObjectListPage>("list_resources", { cluster, kind, opts }).then((page) => {
        page.items.forEach(normalizeSummary);
        return fill(page, { continueToken: null, remaining: null });
      }),
    graph: (cluster: string, namespace: string | null) =>
      call<T.GraphSnapshot>("load_graph", { cluster, namespace }).then((g) => {
        g.objects.forEach(normalizeSummary);
        return g;
      }),
    yaml: (cluster: string, reference: T.ResourceRef) => call<string>("get_yaml", { cluster, reference }),
    apply: (
      cluster: string,
      yaml: string,
      opts: { dryRun?: boolean; force?: boolean; namespace?: string | null } = {},
    ) =>
      call<T.ApplyOutcome>("apply_yaml", {
        cluster,
        yaml,
        dryRun: opts.dryRun ?? false,
        force: opts.force ?? false,
        namespace: opts.namespace ?? null,
      }).then(normalizeOutcome),
    diff: (cluster: string, yaml: string, namespace: string | null) =>
      call<T.DiffItem[]>("diff_yaml", { cluster, yaml, namespace }).then((l) => {
        l.forEach((d) => normalizeRef(d.resource));
        return l;
      }),
    deleteYaml: (cluster: string, yaml: string, namespace: string | null) =>
      call<T.ApplyOutcome>("delete_yaml", { cluster, yaml, namespace }).then(normalizeOutcome),
    replaceYaml: (cluster: string, reference: T.ResourceRef, yaml: string) =>
      call<void>("replace_yaml", { cluster, reference, yaml }),
    delete: (cluster: string, reference: T.ResourceRef, propagation: string | null = null) =>
      call<void>("delete_resource", { cluster, reference, propagation }),
    scale: (cluster: string, reference: T.ResourceRef, replicas: number) =>
      call<void>("scale_resource", { cluster, reference, replicas }),
    restart: (cluster: string, reference: T.ResourceRef) =>
      call<void>("restart_resource", { cluster, reference }),
    setImage: (cluster: string, reference: T.ResourceRef, container: string | null, image: string) =>
      call<void>("set_image", { cluster, reference, container, image }),
    rollback: (cluster: string, reference: T.ResourceRef) =>
      call<void>("rollback_resource", { cluster, reference }),
    cordon: (cluster: string, node: string, on: boolean) => call<void>("cordon_node", { cluster, node, on }),
    drain: (cluster: string, node: string) => call<string[]>("drain_node", { cluster, node }),
    events: (cluster: string, namespace: string | null, involved: T.ResourceRef | null = null) =>
      call<T.EventSummary[]>("load_events", { cluster, namespace, involved }).then((l) => l.map(normalizeEvent)),
    containers: (cluster: string, pod: T.ResourceRef) =>
      call<T.ContainerInfo[]>("load_containers", { cluster, pod }),
    metrics: (cluster: string, namespace: string | null) =>
      call<T.MetricsBundle>("load_metrics", { cluster, namespace }).then((b) => {
        b.nodes.forEach(normalizeMetric);
        b.pods.forEach(normalizeMetric);
        return b;
      }),
  },

  logs: {
    start: (cluster: string, pod: T.ResourceRef, opts: T.LogOptions, onEvent: (ev: T.LogEvent) => void) =>
      call<number>("start_logs", { cluster, pod, opts, onEvent: channel(onEvent) }),
    stop: (id: number) => call<boolean>("stop_logs", { id }),
  },

  exec: {
    start: (
      cluster: string,
      pod: T.ResourceRef,
      container: string | null,
      command: string[],
      size: { cols: number; rows: number },
      onEvent: (ev: T.ExecEvent) => void,
    ) =>
      call<number>("start_exec", {
        cluster,
        pod,
        container,
        command,
        cols: size.cols,
        rows: size.rows,
        onEvent: channel(onEvent),
      }),
    input: (id: number, data: Uint8Array | string) =>
      call<void>("exec_input", {
        id,
        data: bytesToBase64(typeof data === "string" ? new TextEncoder().encode(data) : data),
      }),
    resize: (id: number, cols: number, rows: number) => call<boolean>("exec_resize", { id, cols, rows }),
    stop: (id: number) => call<boolean>("stop_exec", { id }),
  },

  hub: {
    searchImages: (query: string, registry: H.RegistryKind) =>
      call<H.ImageSummary[]>("hub_search_images", { query, registry }),
    listTags: (image: string) => call<H.TagInfo[]>("hub_list_tags", { image }),
    inspect: (image: string) => call<H.ImageDetails>("hub_inspect", { image }),
    searchCharts: (query: string) => call<H.ChartSummary[]>("hub_search_charts", { query }),
    catalog: () => call<H.CatalogApp[]>("hub_catalog"),
    render: (request: H.DeployRequest) => call<string>("hub_render", { request }),
    deploy: (cluster: string, request: H.DeployRequest, dryRun: boolean) =>
      call<T.ApplyOutcome>("hub_deploy", { cluster, request, dryRun }).then(normalizeOutcome),
  },

  updates: {
    list: () => call<U.WatchersBundle>("updates_list"),
    upsertWatcher: (watcher: U.WatcherSpec) => call<U.WatchersBundle>("updates_upsert_watcher", { watcher }),
    removeWatcher: (watcherId: string) => call<U.WatchersBundle>("updates_remove_watcher", { watcherId }),
    check: () => call<U.UpdateFinding[]>("updates_check"),
    apply: (findingId: string) => call<U.RolloutResult>("updates_apply", { findingId }),
    history: () => call<U.RolloutResult[]>("updates_history"),
    scan: (cluster: string, namespace: string | null) =>
      call<U.WorkloadImage[]>("updates_scan", { cluster, namespace }),
    suggest: (cluster: string, namespace: string | null) =>
      call<U.WatcherSpec[]>("updates_suggest", { cluster, namespace }),
    settings: () => call<U.UpdaterSettings>("updates_settings"),
    saveSettings: (settings: U.UpdaterSettings) =>
      call<U.UpdaterSettings>("updates_save_settings", { settings }),
  },

  ai: {
    settings: () => call<T.AiSettingsView>("ai_settings"),
    claudeCodeAccount: () => call<T.ClaudeCodeAccount | null>("ai_claude_code_account"),
    useClaudeCode: () => call<T.ProfileView>("ai_use_claude_code"),
    upsertProfile: (update: T.ProfileUpdate) => call<T.ProfileView>("ai_upsert_profile", { update }),
    removeProfile: (id: string) => call<void>("ai_remove_profile", { id }),
    setActive: (id: string | null) => call<void>("ai_set_active", { id }),
    setGeneral: (toolsEnabled: boolean, maxToolRounds: number, extraInstructions: string) =>
      call<T.AiSettingsView>("ai_set_general", { toolsEnabled, maxToolRounds, extraInstructions }),
    listModels: (draft: T.ProfileUpdate) => call<T.ModelInfo[]>("ai_list_models", { draft }),
    chat: (
      chatId: number,
      messages: T.ChatMessage[],
      context: T.AiContext,
      profileId: string | null,
      onEvent: (ev: T.StreamEvent) => void,
    ) => call<T.ChatOutcome>("ai_chat", { chatId, messages, context, profileId, onEvent: channel(onEvent) }),
    cancel: (chatId: number) => call<boolean>("ai_cancel", { chatId }),
  },

  /** Le backend a fini de recharger les clusters enregistrés, ou l'un d'eux a changé. */
  onClustersChanged: (cb: () => void): Promise<UnlistenFn> => listen("clusters-changed", () => cb()),
  onStartupWarning: (cb: (message: string) => void): Promise<UnlistenFn> =>
    listen<string>("startup-warning", (e) => cb(e.payload)),
};
