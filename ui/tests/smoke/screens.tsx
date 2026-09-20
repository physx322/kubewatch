// Test de fumée : monte chaque écran dans jsdom avec un instantané de cluster
// anonymisé et rapporte toute erreur de rendu (via la frontière d'erreur).
// Le but : attraper les écarts entre les types TypeScript et le JSON réel du
// backend, que le vérificateur de types ne peut pas voir.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { ErrorBoundary } from "@/components/ErrorBoundary";
import { useStore } from "@/app/store";
import { AssistantPanel } from "@/assistant/AssistantPanel";
import { ClustersView } from "@/views/ClustersView";
import { DeployView } from "@/views/DeployView";
import { HubView } from "@/views/HubView";
import { OverviewView } from "@/views/OverviewView";
import { ResourcesView } from "@/views/ResourcesView";
import { SettingsView } from "@/views/SettingsView";
import { TopologyView } from "@/views/TopologyView";
import { UpdatesView } from "@/views/UpdatesView";

const g = globalThis as unknown as { __run: () => Promise<void>; __CALLS: string[] };
g.__CALLS = [];

async function settle(root: Root | null, rounds = 30) {
  void root;
  for (let i = 0; i < rounds; i++) {
    await act(async () => {
      await new Promise((r) => setTimeout(r, 40));
    });
  }
}

function errorText(): string | null {
  const el = document.querySelector(".alert-err");
  if (!el) return null;
  const pre = el.querySelector("pre");
  return (el.querySelector(".mono")?.textContent ?? el.textContent ?? "").slice(0, 300) + (pre ? "\n" + (pre.textContent ?? "").split("\n").slice(0, 6).join("\n") : "");
}

async function mount(label: string, node: ReactNode, after?: () => Promise<void>): Promise<string> {
  useStore.setState({ cluster: "default", namespaceByCluster: { default: null }, selection: null, selectionYaml: null, filter: "" });
  const qc = new QueryClient({ defaultOptions: { queries: { retry: 0 } } });
  document.body.innerHTML = '<div id="root"></div>';
  const root = createRoot(document.getElementById("root")!);
  let thrown: string | null = null;
  try {
    await act(async () => {
      root.render(
        <QueryClientProvider client={qc}>
          <ErrorBoundary label={label} resetKey={1}>{node}</ErrorBoundary>
        </QueryClientProvider>,
      );
    });
    await settle(root);
    if (after) {
      await after();
      await settle(root);
    }
  } catch (e) {
    thrown = String((e as Error)?.stack ?? e).split("\n").slice(0, 6).join("\n");
  }
  const err = errorText() ?? thrown;
  const size = document.body.textContent?.length ?? 0;
  await act(async () => root.unmount());
  return `${err ? "ÉCHEC" : "ok"}  ${label}  (texte rendu : ${size} car.)${err ? "\n    " + err.replace(/\n/g, "\n    ") : ""}`;
}

const click = async (sel: string) => {
  const el = document.querySelector<HTMLElement>(sel);
  if (!el) throw new Error(`élément introuvable : ${sel}`);
  await act(async () => {
    el.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
  });
};

g.__run = async () => {
  const out: string[] = [];
  out.push(await mount("Vue d'ensemble", <OverviewView />));
  out.push(await mount("Clusters", <ClustersView />));
  out.push(
    await mount("Ressources + détail", <ResourcesView />, async () => {
      await click("tbody tr");
      const tabs = [...document.querySelectorAll<HTMLElement>(".tab")];
      for (const t of tabs) {
        if (/Évènements/.test(t.textContent ?? "")) await act(async () => t.dispatchEvent(new window.MouseEvent("click", { bubbles: true })));
      }
    }),
  );
  useStore.setState({ resourcesKind: "deployments" });
  out.push(await mount("Ressources (deployments)", <ResourcesView />, async () => click("tbody tr")));
  useStore.setState({ resourcesKind: "services" });
  out.push(await mount("Ressources (services)", <ResourcesView />, async () => click("tbody tr")));
  useStore.setState({ resourcesKind: "nodes" });
  out.push(await mount("Ressources (nodes)", <ResourcesView />, async () => click("tbody tr")));
  useStore.setState({ resourcesKind: "pods" });
  out.push(await mount("Topologie", <TopologyView />));
  out.push(await mount("Hub", <HubView />));
  out.push(await mount("Déployer", <DeployView />));
  out.push(await mount("Mises à jour", <UpdatesView />));
  out.push(await mount("Réglages", <SettingsView />));
  out.push(await mount("Assistant", <AssistantPanel />));
  console.log(out.join("\n"));
  console.log("commandes appelées :", [...new Set(g.__CALLS)].join(", "));
  const failed = out.filter((l) => l.startsWith("ÉCHEC")).length;
  (globalThis as unknown as { __FAILED: number }).__FAILED = failed;
};
