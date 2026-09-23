// ---------------------------------------------------------------------------
// Dispositions du graphe de topologie et géométrie de la vue.
//
// * par couches : une colonne par famille présente, ordre des lignes par
//   barycentre pour limiter les croisements, puis alignement vertical de
//   chaque nœud sur ses voisins ; déterministe et lisible ;
// * organique : placement par forces (d3-force), les objets liés se rapprochent.
//
// Les positions sont celles du coin supérieur gauche de chaque carte, en
// unités de scène. Les nœuds épinglés ne sont jamais déplacés.
// ---------------------------------------------------------------------------
import {
  forceCollide,
  forceLink,
  forceManyBody,
  forceSimulation,
  forceX,
  forceY,
  type Simulation,
  type SimulationLinkDatum,
  type SimulationNodeDatum,
} from "d3-force";
import { KIND_COLUMN, KIND_ORDER, type Graph } from "./model";

/** Largeur d'une carte, en unités de scène. */
export const NODE_W = 196;
/** Hauteur d'une carte. */
export const NODE_H = 46;
/** Espace horizontal entre deux colonnes. */
export const COL_GAP = 120;
/** Pas vertical entre deux cartes d'une même colonne. */
export const ROW_PITCH = NODE_H + 16;
/** Bornes du zoom. */
export const ZOOM_MIN = 0.08;
export const ZOOM_MAX = 2.5;
/** Distance d'équilibre de la disposition organique. */
export const ORGANIC_K = 170;
/** Au-delà, la disposition organique (quadratique) est refusée. */
export const ORGANIC_MAX_NODES = 1_200;
/** Au-delà, le graphe n'est dessiné qu'après confirmation. */
export const LARGE_NODES = 2_000;
/** En dessous de cette échelle, une carte ne porte plus que son nom. */
export const ZOOM_NAME_ONLY = 0.55;
/** En dessous de cette échelle, une carte ne porte plus aucun texte. */
export const ZOOM_NO_TEXT = 0.3;
/** Taille minimale du nom en zoom arrière, en pixels d'écran. */
export const MIN_NAME_FONT = 9;
/** Sensibilité de la molette : ~16 % par cran de 100 px. */
export const WHEEL_ZOOM = 0.0015;

export interface Pos {
  x: number;
  y: number;
}

export type Positions = Map<string, Pos>;

export type LayoutMode = "layered" | "organic";

/** Transformation scène → écran : `écran = scène × k + (x, y)`. */
export interface ViewTransform {
  x: number;
  y: number;
  k: number;
}

export interface Bounds {
  x: number;
  y: number;
  w: number;
  h: number;
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

function compareOrder(graph: Graph, a: number, b: number): number {
  const na = graph.nodes[a]!;
  const nb = graph.nodes[b]!;
  const ns = (na.namespace ?? "").localeCompare(nb.namespace ?? "");
  if (ns !== 0) return ns;
  const k = KIND_ORDER[na.kind] - KIND_ORDER[nb.kind];
  if (k !== 0) return k;
  return na.name.localeCompare(nb.name);
}

/**
 * Disposition par couches.
 *
 * 1. Une colonne par famille présente (les colonnes vides sont resserrées).
 * 2. Quatre balayages de barycentre pour limiter les croisements.
 * 3. Six tours d'alignement vertical : chaque nœud vise la hauteur moyenne
 *    de ses voisins, sans jamais chevaucher celui du dessus.
 */
export function layeredLayout(graph: Graph, positions: Positions, pinned: Set<string>): void {
  const n = graph.nodes.length;
  if (n === 0) return;

  const used = [...new Set(graph.nodes.map((x) => KIND_COLUMN[x.kind]))].sort((a, b) => a - b);
  const col = graph.nodes.map((x) => Math.max(0, used.indexOf(KIND_COLUMN[x.kind])));
  let columns: number[][] = used.map(() => []);
  col.forEach((c, i) => columns[c]!.push(i));
  for (const column of columns) column.sort((a, b) => compareOrder(graph, a, b));

  const rank = new Float64Array(n);
  const refresh = () => {
    for (const column of columns) column.forEach((i, r) => (rank[i] = r));
  };
  refresh();

  // Barycentre : alterner gauche→droite et droite→gauche.
  for (let sweep = 0; sweep < 4; sweep++) {
    const forward = sweep % 2 === 0;
    const order = columns.map((_, c) => c);
    if (!forward) order.reverse();
    for (const c of order) {
      const keyed = columns[c]!.map((i) => {
        let sum = 0;
        let cnt = 0;
        for (const j of graph.adjacency[i] ?? []) {
          const cj = col[j] ?? 0;
          if (forward ? cj < c : cj > c) {
            sum += rank[j] ?? 0;
            cnt += 1;
          }
        }
        return [i, cnt > 0 ? sum / cnt : (rank[i] ?? 0)] as const;
      });
      keyed.sort((a, b) => a[1] - b[1]);
      columns[c] = keyed.map((k) => k[0]);
      refresh();
    }
  }

  // Alignement vertical sur les voisins.
  const y = Array.from(rank, (r) => r * ROW_PITCH);
  for (let round = 0; round < 6; round++) {
    columns = columns.map((column) => {
      const wanted: [number, number][] = column.map((i) => {
        const nb = graph.adjacency[i] ?? [];
        let target = y[i] ?? 0;
        if (nb.length > 0) target = nb.reduce((s, j) => s + (y[j] ?? 0), 0) / nb.length;
        return [i, target];
      });
      wanted.sort((a, b) => a[1] - b[1]);
      let cursor = -Infinity;
      let placedSum = 0;
      let wantedSum = 0;
      for (const [i, target] of wanted) {
        const placed = Math.max(target, cursor);
        y[i] = placed;
        cursor = placed + ROW_PITCH;
        placedSum += placed;
        wantedSum += target;
      }
      // Recentrer la colonne sur ce qu'elle visait : l'empilement ne pousse
      // que vers le bas, on compense.
      const shift = (wantedSum - placedSum) / Math.max(1, wanted.length);
      for (const [i] of wanted) y[i] = (y[i] ?? 0) + shift;
      return wanted.map((w) => w[0]);
    });
  }

  let top = Infinity;
  for (const v of y) top = Math.min(top, v);
  graph.nodes.forEach((node, i) => {
    if (pinned.has(node.key) && positions.has(node.key)) return;
    positions.set(node.key, { x: (col[i] ?? 0) * (NODE_W + COL_GAP), y: (y[i] ?? 0) - top });
  });
}

/** Rectangle englobant toutes les cartes. */
export function bounds(graph: Graph, positions: Positions): Bounds {
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  for (const node of graph.nodes) {
    const p = positions.get(node.key);
    if (!p) continue;
    x0 = Math.min(x0, p.x);
    y0 = Math.min(y0, p.y);
    x1 = Math.max(x1, p.x + NODE_W);
    y1 = Math.max(y1, p.y + NODE_H);
  }
  if (!Number.isFinite(x0) || x1 <= x0 || y1 <= y0) return { x: 0, y: 0, w: NODE_W, h: NODE_H };
  return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
}

/** Vue qui cadre `b` dans une fenêtre `w × h`, sans jamais dépasser l'échelle 1:1. */
export function fitView(b: Bounds, w: number, h: number): ViewTransform {
  const pad = 80;
  const k = clamp(Math.min(w / (b.w + 2 * pad), h / (b.h + 2 * pad)), ZOOM_MIN, 1);
  const cx = b.x + b.w / 2;
  const cy = b.y + b.h / 2;
  return { k, x: w / 2 - cx * k, y: h / 2 - cy * k };
}

/** Vue centrée sur un point de scène, à l'échelle courante. */
export function centerView(v: ViewTransform, cx: number, cy: number, w: number, h: number): ViewTransform {
  return { k: v.k, x: w / 2 - cx * v.k, y: h / 2 - cy * v.k };
}

/**
 * Marge dessinée de part et d'autre de la fenêtre visible, en fraction de sa
 * taille. Plus elle est large, plus on dessine d'objets hors champ, mais moins
 * un déplacement demande de nouveau rendu.
 */
export const CULL_MARGIN = 0.6;

/**
 * Zone de scène à dessiner pour une vue : la fenêtre visible, élargie de
 * `CULL_MARGIN`. Tant que la fenêtre reste dans cette zone, zoomer ou déplacer
 * ne change que la transformation du groupe — rien n'est à redessiner.
 */
export function cullRect(v: ViewTransform, w: number, h: number): Bounds {
  const sw = w / v.k;
  const sh = h / v.k;
  const mx = sw * CULL_MARGIN;
  const my = sh * CULL_MARGIN;
  return { x: -v.x / v.k - mx, y: -v.y / v.k - my, w: sw + 2 * mx, h: sh + 2 * my };
}

/** Vrai si la fenêtre visible de `v` tient entièrement dans `r`. */
export function viewInside(r: Bounds, v: ViewTransform, w: number, h: number): boolean {
  return (
    -v.x / v.k >= r.x &&
    -v.y / v.k >= r.y &&
    (-v.x + w) / v.k <= r.x + r.w &&
    (-v.y + h) / v.k <= r.y + r.h
  );
}

/** Vrai si la boîte de coins (x0, y0) et (x1, y1) rencontre `r`. */
export function hits(r: Bounds, x0: number, y0: number, x1: number, y1: number): boolean {
  return x1 >= r.x && x0 <= r.x + r.w && y1 >= r.y && y0 <= r.y + r.h;
}

/** Zoom d'un facteur autour d'un point écran, borné. */
export function zoomAt(v: ViewTransform, px: number, py: number, factor: number): ViewTransform {
  const target = clamp(v.k * factor, ZOOM_MIN, ZOOM_MAX);
  const applied = target / v.k;
  if (Math.abs(applied - 1) < 1e-4) return v;
  return { k: target, x: px - (px - v.x) * applied, y: py - (py - v.y) * applied };
}

/** Coupe un texte pour qu'il tienne dans `maxWidth`, à partir d'une largeur moyenne de glyphe. */
export function fitText(text: string, maxWidth: number, fontSize: number): string {
  const avg = fontSize * 0.56;
  const max = Math.floor(maxWidth / avg);
  if (text.length <= max) return text;
  if (max <= 1) return "…";
  return text.slice(0, max - 1) + "…";
}

// --- Disposition organique (d3-force) --------------------------------------

export interface SimNode extends SimulationNodeDatum {
  id: string;
}

export type Sim = Simulation<SimNode, SimulationLinkDatum<SimNode>>;

/**
 * Lance une simulation par forces sur le graphe. Les nœuds partent de leur
 * position courante (centre de la carte) ; les épinglés sont fixés. À chaque
 * pas, `positions` est mis à jour puis `onTick` appelé.
 */
export function startSimulation(graph: Graph, positions: Positions, pinned: Set<string>, onTick: () => void): Sim {
  const nodes: SimNode[] = graph.nodes.map((n) => {
    const p = positions.get(n.key) ?? { x: 0, y: 0 };
    const cx = p.x + NODE_W / 2;
    const cy = p.y + NODE_H / 2;
    const fixed = pinned.has(n.key);
    return { id: n.key, x: cx, y: cy, fx: fixed ? cx : null, fy: fixed ? cy : null };
  });
  const links: SimulationLinkDatum<SimNode>[] = graph.edges.map((e) => ({ source: e.from, target: e.to }));
  const sim = forceSimulation<SimNode>(nodes)
    .force("link", forceLink<SimNode, SimulationLinkDatum<SimNode>>(links).distance(ORGANIC_K).strength(0.6))
    .force("charge", forceManyBody<SimNode>().strength(-900).distanceMax(900))
    .force("collide", forceCollide<SimNode>(NODE_W * 0.5).strength(0.6))
    .force("x", forceX<SimNode>(0).strength(0.03))
    .force("y", forceY<SimNode>(0).strength(0.03))
    .alphaDecay(0.03)
    .on("tick", () => {
      for (const n of nodes) positions.set(n.id, { x: (n.x ?? 0) - NODE_W / 2, y: (n.y ?? 0) - NODE_H / 2 });
      onTick();
    });
  return sim;
}

/** Nœud de simulation par clé. */
export function simNode(sim: Sim, key: string): SimNode | undefined {
  return sim.nodes().find((n) => n.id === key);
}
