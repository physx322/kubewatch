// Faux pont Tauri pour le test de fumée : sert l'instantané de fixtures/ aux
// commandes que les écrans appellent. Rien ne sort du processus.
import * as fs from "node:fs";

const SNAPSHOT_PATH = (globalThis as unknown as { __SNAPSHOT_PATH: string }).__SNAPSHOT_PATH;
const snapshot = JSON.parse(fs.readFileSync(SNAPSHOT_PATH, "utf8")) as { objects: { kind: string; apiVersion: string; namespace: string | null }[]; warnings: string[] };

export class Channel<T> {
  onmessage: (ev: T) => void = () => {};
}

function kinds() {
  const seen = new Map<string, { group: string; version: string; kind: string; plural: string; namespaced: boolean; verbs: string[]; shortNames: string[]; categories: string[] }>();
  for (const o of snapshot.objects) {
    const i = o.apiVersion.indexOf("/");
    const group = i < 0 ? "" : o.apiVersion.slice(0, i);
    const version = i < 0 ? o.apiVersion : o.apiVersion.slice(i + 1);
    const key = `${o.kind}/${group}`;
    if (!seen.has(key)) seen.set(key, { group, version, kind: o.kind, plural: o.kind.toLowerCase() + (o.kind.endsWith("s") ? "es" : "s"), namespaced: o.namespace != null, verbs: ["list", "get"], shortNames: [], categories: [] });
  }
  return [...seen.values()];
}

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  (globalThis as unknown as { __CALLS: string[] }).__CALLS.push(cmd);
  switch (cmd) {
    case "load_graph":
      return snapshot as unknown as T;
    case "list_kinds":
      return kinds() as unknown as T;
    case "list_clusters":
      return [{ name: "default", context: "default", server: "https://k3s", version: "v1.30", platform: "linux/amd64", connected: true, nodeCount: 1, namespaceCount: 5, defaultNamespace: "default", metricsAvailable: false, lastError: null }] as unknown as T;
    case "list_namespaces":
      return [...new Set(snapshot.objects.map((o) => o.namespace).filter((n): n is string => !!n))].sort() as unknown as T;
    case "current_cluster":
      return "default" as unknown as T;
    case "list_resources": {
      const wanted = String(args?.kind ?? "pods");
      const k = kinds().find((x) => x.plural === wanted || x.plural + "." + x.group === wanted);
      const items = snapshot.objects.filter((o) => k ? o.kind === k.kind : false);
      return { items, continueToken: null, remaining: null } as unknown as T;
    }
    case "cluster_overview":
      return { nodesTotal: 2, nodesReady: 2, podsTotal: 34, podsRunning: 30, podsPending: 1, podsFailed: 3, namespaces: 6, cpuCapacityMillis: 8000, cpuUsedMillis: 1200, memoryCapacityBytes: 16e9, memoryUsedBytes: 7e9, workloads: { Deployment: 15, StatefulSet: 3 }, warnings: ["un avertissement de test"], metricsAvailable: true } as unknown as T;
    case "load_events":
      return [{ namespace: "default", name: "e1", reason: "BackOff", message: "Back-off restarting failed container", type: "Warning", count: 12, lastSeen: new Date().toISOString(), involvedKind: "Pod", involvedName: snapshot.objects.find((o) => o.kind === "Pod")?.name ?? "x" }] as unknown as T;
    case "load_containers":
      return [{ name: "app", image: "nginx", ready: true, restartCount: 0, state: "running", init: false }] as unknown as T;
    case "get_yaml":
      return "apiVersion: v1\nkind: Pod\nmetadata:\n  name: x\n" as unknown as T;
    case "load_metrics":
      return { nodes: [], pods: [] } as unknown as T;
    case "hub_catalog":
      return [{ id: "nginx", name: "NGINX", description: "Serveur web", icon: null, category: "Web", image: "nginx:1.27", defaultPort: 80, env: [], needsPvc: false, chart: null, docsUrl: null }] as unknown as T;
    case "updates_list":
      return { watchers: [], findings: [] } as unknown as T;
    case "updates_history":
      return [] as unknown as T;
    case "updates_settings":
      return { githubToken: null, webhookSecret: null, defaultPolicy: { channel: "patch", allowPrerelease: false, ignore: [], autoApply: false, checkIntervalSeconds: 3600 }, schedulerEnabled: true } as unknown as T;
    case "ai_settings":
      return { profiles: [{ id: "p1", name: "Claude", kind: "anthropic", baseUrl: "https://api.anthropic.com", model: "claude-opus-5", apiKeySet: true, maxOutputTokens: 32000, showThinking: false }], activeProfile: "p1", toolsEnabled: true, maxToolRounds: 8, extraInstructions: "" } as unknown as T;
    case "start_logs":
      return 1 as unknown as T;
    case "app_info":
      return { version: "0.1.0-harness", stateDir: "/tmp", warnings: [] } as unknown as T;
    default:
      void args;
      return {} as T;
  }
}
