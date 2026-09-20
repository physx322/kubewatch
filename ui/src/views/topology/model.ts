// ---------------------------------------------------------------------------
// Modèle du graphe de topologie : familles d'objets, couches, liens et
// construction à partir d'un instantané d'objets. Port de la partie « modèle »
// de crates/desktop/src/views/graph.rs.
//
// | Lien       | Source → cible                                   | Origine                                 |
// | ---------- | ------------------------------------------------ | --------------------------------------- |
// | owns       | Deployment → ReplicaSet → Pod, CronJob → Job → … | `owners` (ownerReferences)              |
// | selects    | Service → Pod                                    | `extra.selector` confronté aux labels   |
// | routes     | Ingress → Service                                | `extra.backends`                        |
// | schedules  | Pod → Node                                       | `node`                                  |
// | mounts     | Pod → PersistentVolumeClaim                      | `extra.volumeClaims`                    |
// | uses       | Pod → ConfigMap / Secret                         | `extra.configMaps`, `extra.secrets`     |
// ---------------------------------------------------------------------------
import type { ObjectSummary } from "@/api/types";

/** Familles représentées, dans l'ordre des colonnes et du tri. */
export const NODE_KINDS = [
  "Ingress",
  "Service",
  "Deployment",
  "StatefulSet",
  "DaemonSet",
  "CronJob",
  "ReplicaSet",
  "Job",
  "Pod",
  "PersistentVolumeClaim",
  "ConfigMap",
  "Secret",
  "Node",
] as const;

export type NodeKind = (typeof NODE_KINDS)[number];

const KIND_SET = new Set<string>(NODE_KINDS);

/** Famille correspondant à un kind Kubernetes, `null` s'il n'est pas représenté. */
export function nodeKindOf(kind: string): NodeKind | null {
  return KIND_SET.has(kind) ? (kind as NodeKind) : null;
}

/** Ordre de déclaration, pour trier les nœuds d'une colonne. */
export const KIND_ORDER: Record<NodeKind, number> = Object.fromEntries(NODE_KINDS.map((k, i) => [k, i])) as Record<NodeKind, number>;

/** Libellé court posé sur la carte. */
export const KIND_LABEL: Record<NodeKind, string> = {
  Ingress: "Ingress",
  Service: "Service",
  Deployment: "Deployment",
  StatefulSet: "StatefulSet",
  DaemonSet: "DaemonSet",
  CronJob: "CronJob",
  ReplicaSet: "ReplicaSet",
  Job: "Job",
  Pod: "Pod",
  PersistentVolumeClaim: "PVC",
  ConfigMap: "ConfigMap",
  Secret: "Secret",
  Node: "Nœud",
};

/** Colonne de la disposition par couches. */
export const KIND_COLUMN: Record<NodeKind, number> = {
  Ingress: 0,
  Service: 1,
  Deployment: 2,
  StatefulSet: 2,
  DaemonSet: 2,
  CronJob: 2,
  ReplicaSet: 3,
  Job: 3,
  Pod: 4,
  PersistentVolumeClaim: 5,
  ConfigMap: 5,
  Secret: 5,
  Node: 6,
};

/** Couches affichées : chaque case masque ou montre une famille d'objets. */
export interface Layers {
  ingress: boolean;
  services: boolean;
  /** Deployments, StatefulSets, DaemonSets, CronJobs, Jobs. */
  workloads: boolean;
  /** Masqués par défaut : le Deployment est relié directement à ses pods. */
  replicasets: boolean;
  pods: boolean;
  /** PersistentVolumeClaims. */
  storage: boolean;
  /** ConfigMaps et Secrets, masqués par défaut : ils encombrent vite. */
  config: boolean;
  nodes: boolean;
}

export const DEFAULT_LAYERS: Layers = {
  ingress: true,
  services: true,
  workloads: true,
  replicasets: false,
  pods: true,
  storage: true,
  config: false,
  nodes: true,
};

/** Vrai si la famille est affichée. */
export function layerShows(layers: Layers, kind: NodeKind): boolean {
  switch (kind) {
    case "Ingress":
      return layers.ingress;
    case "Service":
      return layers.services;
    case "Deployment":
    case "StatefulSet":
    case "DaemonSet":
    case "CronJob":
    case "Job":
      return layers.workloads;
    case "ReplicaSet":
      return layers.replicasets;
    case "Pod":
      return layers.pods;
    case "PersistentVolumeClaim":
      return layers.storage;
    case "ConfigMap":
    case "Secret":
      return layers.config;
    case "Node":
      return layers.nodes;
  }
}

export type EdgeKind = "owns" | "selects" | "routes" | "schedules" | "mounts" | "uses";

/** Libellé d'un lien, dans la légende et le panneau de détail. */
export const EDGE_LABEL: Record<EdgeKind, string> = {
  owns: "possède",
  selects: "sélectionne",
  routes: "route vers",
  schedules: "planifié sur",
  mounts: "monte",
  uses: "utilise",
};

/** Libellé d'un lien lu depuis la cible. */
export const EDGE_LABEL_REVERSE: Record<EdgeKind, string> = {
  owns: "possédé par",
  selects: "sélectionné par",
  routes: "routé depuis",
  schedules: "héberge",
  mounts: "monté par",
  uses: "utilisé par",
};

/** Un nœud du graphe : l'essentiel de l'objet, suffisant pour le dessiner. */
export interface GraphNode {
  /** Identité `kind/namespace/nom`. */
  key: string;
  kind: NodeKind;
  namespace: string | null;
  name: string;
  status: string;
  ready: string | null;
  restarts: number | null;
  images: string[];
  node: string | null;
  ageSeconds: number | null;
  apiVersion: string;
  /** Vrai si l'objet a été listé ; faux s'il n'est connu que par référence. */
  known: boolean;
  /** Objet listé, pour le panneau de détail ; absent pour un nœud fictif. */
  summary: ObjectSummary | null;
}

export interface GraphEdge {
  from: number;
  to: number;
  kind: EdgeKind;
}

export interface Graph {
  nodes: GraphNode[];
  edges: GraphEdge[];
  /** Index d'un nœud par sa clé. */
  index: Map<string, number>;
  /** Voisins de chaque nœud, dans les deux sens. */
  adjacency: number[][];
}

export const EMPTY_GRAPH: Graph = { nodes: [], edges: [], index: new Map(), adjacency: [] };

/** Clé d'un nœud. Un nœud du cluster n'a jamais de namespace. */
export function nodeKey(kind: NodeKind, namespace: string | null | undefined, name: string): string {
  return `${kind}/${kind === "Node" ? "" : (namespace ?? "")}/${name}`;
}

function fromSummary(kind: NodeKind, o: ObjectSummary): GraphNode {
  const namespace = kind === "Node" ? null : o.namespace;
  return {
    key: nodeKey(kind, namespace, o.name),
    kind,
    namespace,
    name: o.name,
    status: o.status,
    ready: o.ready,
    restarts: o.restarts,
    images: o.images,
    node: o.node,
    ageSeconds: o.ageSeconds,
    apiVersion: o.apiVersion,
    known: true,
    summary: o,
  };
}

function placeholder(kind: NodeKind, namespace: string | null, name: string): GraphNode {
  const ns = kind === "Node" ? null : namespace;
  return {
    key: nodeKey(kind, ns, name),
    kind,
    namespace: ns,
    name,
    status: "",
    ready: null,
    restarts: null,
    images: [],
    node: null,
    ageSeconds: null,
    apiVersion: "",
    known: false,
    summary: null,
  };
}

/**
 * Vrai si tous les couples du sélecteur se retrouvent dans les labels.
 * Un sélecteur vide ne sélectionne rien (service sans sélecteur, ExternalName).
 */
export function selectorMatches(selector: Record<string, unknown>, labels: Record<string, string>): boolean {
  const entries = Object.entries(selector);
  return entries.length > 0 && entries.every(([k, v]) => typeof v === "string" && labels[k] === v);
}

/** Tableau JSON de chaînes, ou rien. */
function strings(v: unknown): string[] {
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
}

function selectorOf(o: ObjectSummary): Record<string, unknown> | null {
  const sel = o.extra["selector"];
  return sel && typeof sel === "object" && !Array.isArray(sel) ? (sel as Record<string, unknown>) : null;
}

/** Nombre de répliques voulu par un contrôleur. */
function desired(o: ObjectSummary): number {
  const d = o.extra["desired"];
  return typeof d === "number" ? d : 0;
}

class Builder {
  nodes: GraphNode[] = [];
  edges: GraphEdge[] = [];
  index = new Map<string, number>();

  push(node: GraphNode): number {
    const existing = this.index.get(node.key);
    if (existing !== undefined) return existing;
    const i = this.nodes.length;
    this.index.set(node.key, i);
    this.nodes.push(node);
    return i;
  }

  ensure(kind: NodeKind, namespace: string | null, name: string): number {
    const key = nodeKey(kind, namespace, name);
    const existing = this.index.get(key);
    return existing !== undefined ? existing : this.push(placeholder(kind, namespace, name));
  }

  link(from: number, to: number, kind: EdgeKind) {
    if (from !== to) this.edges.push({ from, to, kind });
  }

  dropIsolated() {
    const degree = new Array<number>(this.nodes.length).fill(0);
    for (const e of this.edges) {
      degree[e.from] = (degree[e.from] ?? 0) + 1;
      degree[e.to] = (degree[e.to] ?? 0) + 1;
    }
    const remap = new Array<number>(this.nodes.length).fill(-1);
    const kept: GraphNode[] = [];
    this.nodes.forEach((node, i) => {
      if ((degree[i] ?? 0) > 0) {
        remap[i] = kept.length;
        kept.push(node);
      }
    });
    this.nodes = kept;
    for (const e of this.edges) {
      e.from = remap[e.from] ?? -1;
      e.to = remap[e.to] ?? -1;
    }
    this.index = new Map(this.nodes.map((n, i) => [n.key, i]));
  }

  finish(): Graph {
    const seen = new Set<string>();
    const edges = this.edges.filter((e) => {
      const k = `${e.from}>${e.to}:${e.kind}`;
      if (seen.has(k)) return false;
      seen.add(k);
      return true;
    });
    const adjacency: number[][] = this.nodes.map(() => []);
    for (const e of edges) {
      adjacency[e.from]?.push(e.to);
      adjacency[e.to]?.push(e.from);
    }
    return { nodes: this.nodes, edges, index: this.index, adjacency };
  }
}

/**
 * Construit le graphe à partir d'un instantané.
 *
 * Les ReplicaSets masqués sont « repliés » : leurs pods sont rattachés au
 * Deployment qui les possède. Un ReplicaSet sans contrôleur reste visible
 * quoi qu'il arrive : c'est alors lui la charge de travail.
 */
export function buildGraph(objects: ObjectSummary[], layers: Layers, hideIsolated: boolean): Graph {
  const raws: { kind: NodeKind; o: ObjectSummary }[] = [];
  for (const o of objects) {
    const kind = nodeKindOf(o.kind);
    if (kind) raws.push({ kind, o });
  }

  const byKey = new Map<string, number>();
  raws.forEach(({ kind, o }, i) => byKey.set(nodeKey(kind, o.namespace, o.name), i));

  // ReplicaSets possédant au moins un pod : les seuls qui méritent un nœud
  // quand la couche est affichée (chaque rollout laisse des ReplicaSets vides).
  const rsWithPods = new Set<number>();
  for (const { kind, o } of raws) {
    if (kind !== "Pod") continue;
    for (const owner of o.owners) {
      if (owner.kind !== "ReplicaSet") continue;
      const i = byKey.get(nodeKey("ReplicaSet", o.namespace, owner.name));
      if (i !== undefined) rsWithPods.add(i);
    }
  }

  const visible = raws.map(({ kind, o }, i) => {
    if (!layerShows(layers, kind)) return kind === "ReplicaSet" && !o.owners.some((w) => w.controller);
    if (kind === "ReplicaSet") return rsWithPods.has(i) || desired(o) > 0;
    return true;
  });

  const b = new Builder();
  const nodeOf: (number | undefined)[] = raws.map(({ kind, o }, i) => (visible[i] ? b.push(fromSummary(kind, o)) : undefined));

  // Propriété : on remonte la chaîne tant que le propriétaire est masqué.
  raws.forEach(({ o }, i) => {
    const child = nodeOf[i];
    if (child === undefined) return;
    for (const owner of o.owners) {
      const ownerKind = nodeKindOf(owner.kind);
      // Pods statiques : possédés par leur nœud, le lien de planification dit déjà tout.
      if (!ownerKind || ownerKind === "Node") continue;
      let cur = byKey.get(nodeKey(ownerKind, o.namespace, owner.name));
      let depth = 0;
      while (cur !== undefined) {
        const parent = nodeOf[cur];
        if (parent !== undefined) {
          b.link(parent, child, "owns");
          break;
        }
        depth += 1;
        if (depth > 3) break;
        const parentObj = raws[cur]?.o;
        if (!parentObj) break;
        const w = parentObj.owners.find((x) => x.controller) ?? parentObj.owners[0];
        const wk = w ? nodeKindOf(w.kind) : null;
        cur = w && wk ? byKey.get(nodeKey(wk, parentObj.namespace, w.name)) : undefined;
      }
    }
  });

  // Services → pods, par sélecteur.
  if (layers.services && layers.pods) {
    const pods = raws.map((r, i) => ({ ...r, i })).filter((r) => r.kind === "Pod");
    raws.forEach(({ kind, o: so }, si) => {
      if (kind !== "Service") return;
      const src = nodeOf[si];
      if (src === undefined) return;
      const selector = selectorOf(so);
      if (!selector || Object.keys(selector).length === 0) return;
      for (const p of pods) {
        if (p.o.namespace !== so.namespace) continue;
        const dst = nodeOf[p.i];
        if (dst === undefined) continue;
        if (selectorMatches(selector, p.o.labels)) b.link(src, dst, "selects");
      }
    });
  }

  // Ingress → services. Un backend absent devient un nœud fictif : c'est
  // précisément le genre d'erreur que l'on vient voir ici.
  if (layers.ingress && layers.services) {
    raws.forEach(({ kind, o }, i) => {
      if (kind !== "Ingress") return;
      const src = nodeOf[i];
      if (src === undefined) return;
      for (const name of strings(o.extra["backends"])) {
        b.link(src, b.ensure("Service", o.namespace, name), "routes");
      }
    });
  }

  // Pod → nœud, volumes, configuration.
  raws.forEach(({ kind, o }, i) => {
    if (kind !== "Pod") return;
    const src = nodeOf[i];
    if (src === undefined) return;
    if (layers.nodes && o.node) {
      b.link(src, b.ensure("Node", null, o.node), "schedules");
    }
    if (layers.storage) {
      for (const claim of strings(o.extra["volumeClaims"])) {
        b.link(src, b.ensure("PersistentVolumeClaim", o.namespace, claim), "mounts");
      }
    }
    if (layers.config) {
      for (const name of strings(o.extra["configMaps"])) {
        b.link(src, b.ensure("ConfigMap", o.namespace, name), "uses");
      }
      for (const name of strings(o.extra["secrets"])) {
        b.link(src, b.ensure("Secret", o.namespace, name), "uses");
      }
    }
  });

  if (hideIsolated) b.dropIsolated();
  return b.finish();
}

/** Vrai si le nœud répond au filtre de la barre supérieure (déjà en minuscules). */
export function nodeMatches(node: GraphNode, needle: string): boolean {
  const hit = (s: string | null) => !!s && s.toLowerCase().includes(needle);
  return hit(node.name) || hit(node.namespace) || hit(node.status) || hit(KIND_LABEL[node.kind]) || hit(node.kind);
}
