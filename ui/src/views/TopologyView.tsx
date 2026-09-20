// Écran Topologie : le graphe des objets du cluster et de leurs liens.
//
// Chaque objet est une carte, chaque relation connue un lien (voir
// topology/model.ts). Deux dispositions : par couches (une colonne par
// famille, de l'Ingress au nœud) ou organique (forces, d3-force). La vue se
// déplace au glisser et zoome à la molette ; les cartes se déplacent à la
// main et restent épinglées jusqu'à « Réorganiser ». Les positions survivent
// aux rafraîchissements : seul un objet nouveau déclenche un nouveau placement.
import {
  ArrowsClockwise,
  ArrowSquareOut,
  Copy,
  CornersOut,
  Crosshair,
  Cube,
  Info,
  PushPin,
  PushPinSlash,
  TreeStructure,
  Warning,
  X,
} from "@phosphor-icons/react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useReducer, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { api } from "@/api/client";
import type { ResourceKind } from "@/api/types";
import { useKinds } from "@/app/queries";
import { useNamespace, useStore } from "@/app/store";
import { Alert, Badge, EmptyState, ReadyBadge, Spinner, StatusBadge } from "@/components/Basics";
import { ContextMenu, type MenuItem } from "@/components/Menu";
import { age, count, statusTone, type Tone } from "@/lib/format";
import { pluralOf } from "@/lib/kinds";
import {
  bounds,
  centerView,
  fitText,
  fitView,
  LARGE_NODES,
  layeredLayout,
  MIN_NAME_FONT,
  NODE_H,
  NODE_W,
  ORGANIC_MAX_NODES,
  simNode,
  startSimulation,
  WHEEL_ZOOM,
  ZOOM_NAME_ONLY,
  ZOOM_NO_TEXT,
  zoomAt,
  type LayoutMode,
  type Positions,
  type Sim,
  type ViewTransform,
} from "./topology/layout";
import {
  buildGraph,
  DEFAULT_LAYERS,
  EDGE_LABEL,
  EDGE_LABEL_REVERSE,
  EMPTY_GRAPH,
  KIND_LABEL,
  nodeMatches,
  type EdgeKind,
  type Graph,
  type GraphNode,
  type Layers,
} from "./topology/model";
import "./Topology.css";

/** Rafraîchissement automatique de la topologie. */
const GRAPH_REFRESH_MS = 15_000;

/**
 * Ce qui survit à un passage par un autre écran, le temps de la session
 * sans passer par le store global :
 * les préférences, et par cluster la scène (positions, épingles, cadrage,
 * sélection).
 */
const sessionPrefs = { layers: DEFAULT_LAYERS, mode: "layered" as LayoutMode, hideIsolated: false, showLegend: false };

interface Scene {
  positions: Positions;
  pinned: Set<string>;
  view: ViewTransform | null;
  selected: string | null;
  /** Vrai si la disposition organique se stabilisait encore au démontage. */
  settling: boolean;
}

const sessionScenes = new Map<string, Scene>();

function sceneOf(cluster: string): Scene {
  let scene = sessionScenes.get(cluster);
  if (!scene) {
    scene = { positions: new Map(), pinned: new Set(), view: null, selected: null, settling: false };
    sessionScenes.set(cluster, scene);
  }
  return scene;
}

// ---------------------------------------------------------------------------
// Écran : données, préférences, barre d'outils
// ---------------------------------------------------------------------------

export function TopologyView() {
  const cluster = useStore((s) => s.cluster);
  const namespace = useNamespace();
  const autoRefresh = useStore((s) => s.autoRefresh);
  const qc = useQueryClient();
  const kinds = useKinds(cluster);

  const [layers, setLayers] = useState<Layers>(sessionPrefs.layers);
  const [mode, setMode] = useState<LayoutMode>(sessionPrefs.mode);
  const [hideIsolated, setHideIsolated] = useState(sessionPrefs.hideIsolated);
  const [showLegend, setShowLegend] = useState(sessionPrefs.showLegend);
  const [fitTick, setFitTick] = useState(0);
  const [relayoutTick, setRelayoutTick] = useState(0);
  const [showLarge, setShowLarge] = useState(false);

  const snapshot = useQuery({
    queryKey: ["graph", cluster, namespace],
    queryFn: () => api.resources.graph(cluster!, namespace),
    enabled: !!cluster,
    refetchInterval: autoRefresh ? GRAPH_REFRESH_MS : false,
    // Garder l'instantané précédent pendant une relecture, mais jamais celui d'un autre cluster.
    placeholderData: (prev, prevQuery) => (prevQuery?.queryKey[1] === cluster ? prev : undefined),
  });

  useEffect(() => setShowLarge(false), [cluster, namespace]);
  useEffect(() => {
    Object.assign(sessionPrefs, { layers, mode, hideIsolated, showLegend });
  }, [layers, mode, hideIsolated, showLegend]);

  const objects = snapshot.data?.objects;
  const warnings = snapshot.data?.warnings ?? [];
  const graph = useMemo(() => (objects ? buildGraph(objects, layers, hideIsolated) : EMPTY_GRAPH), [objects, layers, hideIsolated]);
  const organicOk = graph.nodes.length <= ORGANIC_MAX_NODES;
  const effectiveMode: LayoutMode = mode === "organic" && organicOk ? "organic" : "layered";

  const refresh = () => qc.invalidateQueries({ queryKey: ["graph", cluster] });
  const chooseMode = (m: LayoutMode) => {
    if (m !== mode) setMode(m);
  };
  const layer = (id: keyof Layers, label: string, title?: string) => (
    <label className="checkbox" title={title}>
      <input type="checkbox" checked={layers[id]} onChange={(e) => setLayers({ ...layers, [id]: e.target.checked })} />
      {label}
    </label>
  );

  if (!cluster) {
    return (
      <div className="page">
        <EmptyState icon={Cube} title="Aucun cluster sélectionné" hint="Choisissez un cluster dans la barre supérieure, ou ajoutez-en un dans Réglages." />
      </div>
    );
  }

  let body: React.ReactNode;
  if (!snapshot.data) {
    body = snapshot.error ? null : (
      <div className="empty">
        <Spinner label="Lecture de la topologie…" />
      </div>
    );
  } else if (graph.nodes.length === 0) {
    const scope = namespace ? `dans le namespace « ${namespace} »` : "dans ce cluster";
    body = (
      <EmptyState
        icon={TreeStructure}
        title={`Aucun objet à représenter ${scope}`}
        hint={
          objects && objects.length > 0
            ? `${count(objects.length, "objet")} lu(s), mais aucun ne correspond aux couches affichées.`
            : "Aucun pod, service ni charge de travail n'a été listé."
        }
        action={
          <button className="btn btn-sm" onClick={refresh}>
            <ArrowsClockwise size={14} /> Relire
          </button>
        }
      />
    );
  } else if (graph.nodes.length > LARGE_NODES && !showLarge) {
    body = (
      <div className="p-16">
        <Alert tone="warn">
          <Warning size={18} />
          <div className="col gap-4">
            <strong>{count(graph.nodes.length, "objet")} à représenter : le graphe serait trop dense pour rester lisible.</strong>
            <span>
              Choisissez un namespace dans la barre supérieure, ou masquez des couches (pods, volumes, configuration) pour
              réduire le nombre d'objets.
            </span>
            <div className="row mt-8">
              <button className="btn btn-sm" onClick={() => setShowLarge(true)}>
                Afficher quand même
              </button>
            </div>
          </div>
        </Alert>
      </div>
    );
  } else {
    body = (
      <GraphArea
        key={cluster}
        cluster={cluster}
        graph={graph}
        mode={effectiveMode}
        kinds={kinds.data}
        fitTick={fitTick}
        relayoutTick={relayoutTick}
      />
    );
  }

  return (
    <div className="topo fill">
      <div className="toolbar wrap">
        <button className="btn btn-sm" onClick={refresh} title="Relire la topologie">
          <ArrowsClockwise size={14} /> Rafraîchir
        </button>
        {snapshot.isFetching && <Spinner />}
        <span className="vdivider" />
        <span className="muted small">Disposition</span>
        <div className="btn-group">
          <button
            className={`btn btn-sm ${mode === "layered" ? "active" : ""}`}
            onClick={() => chooseMode("layered")}
            title="Une colonne par famille d'objets, de l'Ingress au nœud"
          >
            Couches
          </button>
          <button
            className={`btn btn-sm ${mode === "organic" ? "active" : ""}`}
            disabled={!organicOk}
            onClick={() => chooseMode("organic")}
            title={organicOk ? "Placement par forces : les objets liés se rapprochent" : `Indisponible au-delà de ${ORGANIC_MAX_NODES} objets`}
          >
            Organique
          </button>
        </div>
        <span className="vdivider" />
        <button className="btn btn-sm" onClick={() => setFitTick((t) => t + 1)} title="Cadrer tout le graphe dans la fenêtre">
          <CornersOut size={14} /> Ajuster
        </button>
        <button
          className="btn btn-sm"
          onClick={() => setRelayoutTick((t) => t + 1)}
          title="Recalculer la disposition et détacher les nœuds déplacés à la main"
        >
          Réorganiser
        </button>
        <span className="vdivider" />
        <span className="muted small nowrap">
          {count(graph.nodes.length, "objet")} · {count(graph.edges.length, "lien")}
        </span>
        {warnings.length > 0 && (
          <Badge tone="warn" title={warnings.join("\n")}>
            <Warning size={12} /> {count(warnings.length, "type")} non lu(s)
          </Badge>
        )}
        <span className="grow" />
      </div>
      <div className="topo-layers">
        <span className="muted small">Couches</span>
        {layer("ingress", "Ingress")}
        {layer("services", "Services")}
        {layer("workloads", "Charges", "Deployments, StatefulSets, DaemonSets, CronJobs, Jobs")}
        {layer("replicasets", "ReplicaSets", "Masqués, les Deployments sont reliés directement à leurs pods")}
        {layer("pods", "Pods")}
        {layer("storage", "Volumes", "PersistentVolumeClaims montés par les pods")}
        {layer("config", "Config", "ConfigMaps et Secrets référencés par les pods")}
        {layer("nodes", "Nœuds")}
        <span className="vdivider" />
        <label className="checkbox" title="Ne garder que les objets reliés à au moins un autre">
          <input type="checkbox" checked={hideIsolated} onChange={(e) => setHideIsolated(e.target.checked)} />
          Masquer les isolés
        </label>
        <label className="checkbox">
          <input type="checkbox" checked={showLegend} onChange={(e) => setShowLegend(e.target.checked)} />
          Légende
        </label>
      </div>
      {showLegend && <Legend />}
      {snapshot.error && (
        <div style={{ padding: 8 }}>
          <Alert tone="err">
            <span className="grow">{(snapshot.error as Error).message}</span>
            <button className="btn btn-sm" onClick={refresh}>
              Relire
            </button>
          </Alert>
        </div>
      )}
      {body}
    </div>
  );
}

/** Légende : couleurs de statut, styles de lien, gestes. */
function Legend() {
  const swatch = (tone: Tone, text: string) => (
    <span className="row gap-4">
      <span className={`topo-swatch tone-${tone}`} />
      {text}
    </span>
  );
  const line = (kind: EdgeKind, text: string) => (
    <span className="row gap-4">
      <svg width="28" height="10" className="topo-legend-line" aria-hidden="true">
        <path className={`topo-edge e-${kind}`} d="M1 5H27" />
      </svg>
      {text}
    </span>
  );
  return (
    <div className="topo-legend">
      {swatch("ok", "en marche")}
      {swatch("warn", "transitoire")}
      {swatch("err", "en erreur")}
      {swatch("neutral", "inconnu ou non listé")}
      <span className="vdivider" />
      {line("owns", EDGE_LABEL.owns)}
      {line("selects", `${EDGE_LABEL.selects} · ${EDGE_LABEL.routes}`)}
      {line("schedules", `${EDGE_LABEL.schedules} · ${EDGE_LABEL.mounts} · ${EDGE_LABEL.uses}`)}
      <span className="vdivider" />
      <span>clic : détail · glisser : déplacer · molette : zoom · fond : déplacer la vue · clic droit : menu</span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// La scène : positions, vue, interactions, panneau de détail
// ---------------------------------------------------------------------------

interface LayoutState {
  positions: Positions;
  pinned: Set<string>;
  fitPending: boolean;
  sim: Sim | null;
}

interface DragState {
  kind: "pan" | "node";
  key: string;
  pointerId: number;
  sx: number;
  sy: number;
  ox: number;
  oy: number;
  moved: boolean;
}

type TextMode = "full" | "name" | "none";

function keyAt(target: EventTarget | null): string | null {
  return target instanceof Element ? (target.closest("[data-key]")?.getAttribute("data-key") ?? null) : null;
}

function GraphArea({
  cluster,
  graph,
  mode,
  kinds,
  fitTick,
  relayoutTick,
}: {
  cluster: string;
  graph: Graph;
  mode: LayoutMode;
  kinds: ResourceKind[] | undefined;
  fitTick: number;
  relayoutTick: number;
}) {
  const filter = useStore((s) => s.filter);
  const toast = useStore((s) => s.toast);
  const containerRef = useRef<HTMLDivElement>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const scene = sceneOf(cluster);
  const layout = useRef<LayoutState>({ positions: scene.positions, pinned: scene.pinned, fitPending: scene.view === null, sim: null });
  const [, bump] = useReducer((n: number) => n + 1, 0);
  const [size, setSize] = useState({ w: 0, h: 0 });
  const [view, setView] = useState<ViewTransform>(scene.view ?? { x: 0, y: 0, k: 1 });
  const viewRef = useRef(view);
  viewRef.current = view;
  const [hovered, setHovered] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(scene.selected);
  useEffect(() => {
    scene.view = view;
    scene.selected = selected;
  }, [scene, view, selected]);
  const [menu, setMenu] = useState<{ x: number; y: number; key: string } | null>(null);
  const [dragging, setDragging] = useState<"pan" | "node" | null>(null);
  const dragRef = useRef<DragState | null>(null);
  const mounted = useRef(false);

  // --- Taille du conteneur.
  useLayoutEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const measure = () => setSize({ w: el.clientWidth, h: el.clientHeight });
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const stopSim = () => {
    layout.current.sim?.stop();
    layout.current.sim = null;
  };
  const runSim = () => {
    const L = layout.current;
    const sim = startSimulation(graph, L.positions, L.pinned, bump);
    sim.on("end", () => {
      if (L.sim === sim) L.sim = null;
    });
    L.sim = sim;
  };

  // --- Reconstruction : retirer les nœuds disparus, placer les nouveaux.
  const sync = useCallback(
    (reset: boolean) => {
      const L = layout.current;
      if (reset) {
        stopSim();
        L.positions.clear();
        L.pinned.clear();
      }
      for (const k of [...L.positions.keys()]) if (!graph.index.has(k)) L.positions.delete(k);
      for (const k of [...L.pinned]) if (!graph.index.has(k)) L.pinned.delete(k);
      const fresh = graph.nodes.some((n) => !L.positions.has(n.key));
      if (fresh) {
        const first = L.positions.size === 0;
        stopSim();
        if (mode === "layered") {
          layeredLayout(graph, L.positions, L.pinned);
        } else {
          // Les nouveaux venus partent de leur place « par couches », les
          // autres ne bougent pas : les forces feront le reste.
          const seed: Positions = new Map();
          layeredLayout(graph, seed, new Set());
          for (const n of graph.nodes) {
            if (!L.positions.has(n.key)) L.positions.set(n.key, seed.get(n.key) ?? { x: 0, y: 0 });
          }
          runSim();
        }
        if (first) L.fitPending = true;
      }
      if (reset) L.fitPending = true;
      bump();
    },
    [graph, mode],
  );

  useLayoutEffect(() => {
    if (mounted.current) sync(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode]);
  useLayoutEffect(() => {
    if (mounted.current && relayoutTick > 0) sync(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [relayoutTick]);
  useLayoutEffect(() => {
    sync(false);
    mounted.current = true;
  }, [sync]);
  useEffect(() => {
    // Remontage : reprendre une stabilisation organique interrompue.
    if (scene.settling && mode === "organic" && !layout.current.sim) runSim();
    scene.settling = false;
    return () => {
      // Démontage (changement d'écran, ou remontage en StrictMode) : la scène
      // reste en session, seule la simulation s'arrête.
      scene.settling = layout.current.sim !== null;
      stopSim();
      mounted.current = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // --- Cadrage demandé : dès que la taille est connue.
  useEffect(() => {
    if (fitTick > 0) {
      layout.current.fitPending = true;
      bump();
    }
  }, [fitTick]);
  useLayoutEffect(() => {
    const L = layout.current;
    if (!L.fitPending || size.w <= 0 || size.h <= 0 || graph.nodes.length === 0) return;
    L.fitPending = false;
    setView(fitView(bounds(graph, L.positions), size.w, size.h));
  });

  // --- Sélection orpheline.
  useEffect(() => {
    if (selected && !graph.index.has(selected)) setSelected(null);
  }, [graph, selected]);

  // --- Échap ferme le détail.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || menu) return;
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
      setSelected(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [menu]);

  // --- Molette : zoom autour du pointeur (écouteur natif, non passif).
  useEffect(() => {
    const el = svgRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const rect = el.getBoundingClientRect();
      const delta = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaMode === 2 ? e.deltaY * 100 : e.deltaY;
      const factor = Math.exp(-delta * WHEEL_ZOOM * (e.ctrlKey ? 2 : 1));
      setView((v) => zoomAt(v, e.clientX - rect.left, e.clientY - rect.top, factor));
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  // --- Pointeur : glisser le fond déplace la vue, glisser une carte la déplace
  // (et l'épingle), un clic sans mouvement sélectionne.
  const onPointerDown = (e: ReactPointerEvent<SVGSVGElement>) => {
    if (e.button !== 0) return;
    setMenu(null);
    const key = keyAt(e.target);
    svgRef.current?.setPointerCapture(e.pointerId);
    if (key) {
      const p = layout.current.positions.get(key) ?? { x: 0, y: 0 };
      dragRef.current = { kind: "node", key, pointerId: e.pointerId, sx: e.clientX, sy: e.clientY, ox: p.x, oy: p.y, moved: false };
    } else {
      const v = viewRef.current;
      dragRef.current = { kind: "pan", key: "", pointerId: e.pointerId, sx: e.clientX, sy: e.clientY, ox: v.x, oy: v.y, moved: false };
    }
  };
  const onPointerMove = (e: ReactPointerEvent<SVGSVGElement>) => {
    const d = dragRef.current;
    if (!d) return;
    const dx = e.clientX - d.sx;
    const dy = e.clientY - d.sy;
    if (!d.moved) {
      if (Math.hypot(dx, dy) < 3) return;
      d.moved = true;
      setDragging(d.kind);
      if (d.kind === "node") layout.current.sim?.alphaTarget(0.3).restart();
    }
    if (d.kind === "pan") {
      setView((v) => ({ ...v, x: d.ox + dx, y: d.oy + dy }));
      return;
    }
    const L = layout.current;
    const k = viewRef.current.k;
    const p = { x: d.ox + dx / k, y: d.oy + dy / k };
    L.positions.set(d.key, p);
    L.pinned.add(d.key);
    const sn = L.sim ? simNode(L.sim, d.key) : undefined;
    if (sn) {
      sn.fx = p.x + NODE_W / 2;
      sn.fy = p.y + NODE_H / 2;
    }
    bump();
  };
  const onPointerUp = () => {
    const d = dragRef.current;
    if (!d) return;
    dragRef.current = null;
    setDragging(null);
    try {
      svgRef.current?.releasePointerCapture(d.pointerId);
    } catch {
      // déjà relâché
    }
    if (!d.moved) {
      setSelected(d.kind === "node" ? d.key : null);
    } else if (d.kind === "node") {
      layout.current.sim?.alphaTarget(0);
    }
  };
  const onPointerOver = (e: ReactPointerEvent<SVGSVGElement>) => {
    if (dragRef.current) return;
    setHovered(keyAt(e.target));
  };
  const onContextMenu = (e: React.MouseEvent<SVGSVGElement>) => {
    e.preventDefault();
    const key = keyAt(e.target);
    if (key) setMenu({ x: e.clientX, y: e.clientY, key });
  };

  // --- Actions.
  const openInResources = (node: GraphNode) =>
    useStore.getState().goToResource({ kind: pluralOf(node.kind, kinds), name: node.name, namespace: node.namespace });
  const centerOn = (key: string) => {
    const p = layout.current.positions.get(key);
    if (p) setView((v) => centerView(v, p.x + NODE_W / 2, p.y + NODE_H / 2, size.w, size.h));
  };
  const togglePin = (key: string) => {
    const L = layout.current;
    const sn = L.sim ? simNode(L.sim, key) : undefined;
    if (L.pinned.has(key)) {
      L.pinned.delete(key);
      if (sn) {
        sn.fx = null;
        sn.fy = null;
        L.sim?.alpha(Math.max(L.sim.alpha(), 0.2)).restart();
      }
    } else {
      L.pinned.add(key);
      if (sn) {
        sn.fx = sn.x;
        sn.fy = sn.y;
      }
    }
    bump();
  };
  const copyName = (name: string) =>
    navigator.clipboard
      .writeText(name)
      .then(() => toast("ok", "Nom copié.", name))
      .catch(() => toast("err", "Copie impossible.", name));
  const focusOn = (key: string) => {
    setSelected(key);
    centerOn(key);
  };

  // --- Mise en avant : nœud survolé, sinon nœud sélectionné ; filtre global.
  const activeKey = hovered ?? selected;
  const activeIdx = activeKey != null ? graph.index.get(activeKey) : undefined;
  const neighbourhood = useMemo(() => {
    if (activeIdx === undefined) return null;
    const s = new Set<number>([activeIdx]);
    for (const j of graph.adjacency[activeIdx] ?? []) s.add(j);
    return s;
  }, [graph, activeIdx]);
  const needle = filter.trim().toLowerCase();
  const matched = useMemo(() => {
    if (!needle) return null;
    const direct = graph.nodes.map((n) => nodeMatches(n, needle));
    const s = new Set<number>();
    graph.nodes.forEach((_, i) => {
      if (direct[i] || (graph.adjacency[i] ?? []).some((j) => direct[j])) s.add(i);
    });
    return s;
  }, [graph, needle]);
  const dimmed = (i: number) => (neighbourhood !== null && !neighbourhood.has(i)) || (matched !== null && !matched.has(i));

  const L = layout.current;
  const P = L.positions;
  const textMode: TextMode = view.k < ZOOM_NO_TEXT ? "none" : view.k < ZOOM_NAME_ONLY ? "name" : "full";
  const nameSize = textMode === "name" ? Math.round(Math.max(13, MIN_NAME_FONT / view.k) * 2) / 2 : 13;
  const layered = mode === "layered";

  const menuNode = menu ? graph.nodes[graph.index.get(menu.key) ?? -1] : undefined;
  const menuItems: MenuItem[] = menuNode
    ? [
        { label: "Ouvrir le détail", icon: <Info size={14} />, onSelect: () => setSelected(menuNode.key) },
        { label: "Ouvrir dans Ressources", icon: <ArrowSquareOut size={14} />, onSelect: () => openInResources(menuNode) },
        { label: "Centrer la vue ici", icon: <Crosshair size={14} />, onSelect: () => centerOn(menuNode.key) },
        {
          label: L.pinned.has(menuNode.key) ? "Détacher" : "Épingler",
          icon: L.pinned.has(menuNode.key) ? <PushPinSlash size={14} /> : <PushPin size={14} />,
          onSelect: () => togglePin(menuNode.key),
        },
        { separator: true, label: "" },
        { label: "Copier le nom", icon: <Copy size={14} />, onSelect: () => copyName(menuNode.name) },
      ]
    : [];

  const selectedNode = selected != null ? graph.nodes[graph.index.get(selected) ?? -1] : undefined;
  const hoveredNode = hovered != null && !dragging && !menu ? graph.nodes[graph.index.get(hovered) ?? -1] : undefined;
  const hoveredPos = hoveredNode ? P.get(hoveredNode.key) : undefined;

  return (
    <div className="topo-area">
      <div ref={containerRef} className="topo-canvas">
        <svg
          ref={svgRef}
          className={`topo-svg${dragging === "pan" ? " panning" : ""}${dragging === "node" ? " moving" : ""}`}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={onPointerUp}
          onPointerOver={onPointerOver}
          onPointerLeave={() => setHovered(null)}
          onContextMenu={onContextMenu}
        >
          <g transform={`translate(${view.x} ${view.y}) scale(${view.k})`}>
            <g className="topo-edges">
              {graph.edges.map((e, i) => {
                const a = P.get(graph.nodes[e.from]?.key ?? "");
                const b = P.get(graph.nodes[e.to]?.key ?? "");
                if (!a || !b) return null;
                const active = activeIdx !== undefined && (e.from === activeIdx || e.to === activeIdx);
                const state = active ? "active" : dimmed(e.from) || dimmed(e.to) ? "dim" : "";
                return <TopoEdge key={i} x1={a.x} y1={a.y} x2={b.x} y2={b.y} kind={e.kind} layered={layered} state={state} />;
              })}
            </g>
            <g className="topo-nodes">
              {graph.nodes.map((n, i) => {
                const p = P.get(n.key);
                if (!p) return null;
                return (
                  <TopoNode
                    key={n.key}
                    node={n}
                    x={p.x}
                    y={p.y}
                    focused={selected === n.key}
                    hovered={hovered === n.key}
                    dim={dimmed(i)}
                    pinned={L.pinned.has(n.key)}
                    textMode={textMode}
                    nameSize={nameSize}
                  />
                );
              })}
            </g>
          </g>
        </svg>
        {hoveredNode && hoveredPos && (
          <Tooltip
            node={hoveredNode}
            x={view.x + hoveredPos.x * view.k}
            top={view.y + hoveredPos.y * view.k}
            bottom={view.y + (hoveredPos.y + NODE_H) * view.k}
            width={size.w}
            height={size.h}
          />
        )}
      </div>
      {selectedNode && (
        <DetailPanel
          node={selectedNode}
          graph={graph}
          pinned={L.pinned.has(selectedNode.key)}
          onClose={() => setSelected(null)}
          onOpen={() => openInResources(selectedNode)}
          onCenter={() => centerOn(selectedNode.key)}
          onTogglePin={() => togglePin(selectedNode.key)}
          onSelect={focusOn}
        />
      )}
      <ContextMenu at={menu} items={menuItems} onClose={() => setMenu(null)} />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Dessin
// ---------------------------------------------------------------------------

const r1 = (v: number) => Math.round(v * 10) / 10;

/** Un lien : courbe de Bézier entre bords en disposition par couches, segment centre à centre sinon. */
const TopoEdge = memo(function TopoEdge({
  x1,
  y1,
  x2,
  y2,
  kind,
  layered,
  state,
}: {
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  kind: EdgeKind;
  layered: boolean;
  state: string;
}) {
  const fcx = x1 + NODE_W / 2;
  const fcy = y1 + NODE_H / 2;
  const tcx = x2 + NODE_W / 2;
  const tcy = y2 + NODE_H / 2;
  let d: string;
  if (layered) {
    const rightwards = tcx >= fcx;
    const ax = rightwards ? x1 + NODE_W : x1;
    const bx = rightwards ? x2 : x2 + NODE_W;
    const sign = rightwards ? 1 : -1;
    const dx = Math.max(Math.abs(bx - ax) * 0.5, 30);
    d = `M${r1(ax)} ${r1(fcy)}C${r1(ax + sign * dx)} ${r1(fcy)} ${r1(bx - sign * dx)} ${r1(tcy)} ${r1(bx)} ${r1(tcy)}`;
  } else {
    d = `M${r1(fcx)} ${r1(fcy)}L${r1(tcx)} ${r1(tcy)}`;
  }
  return <path className={`topo-edge e-${kind}${state ? " " + state : ""}`} d={d} />;
});

const SMALL = 11;
const BODY = 13;
const LEFT = 12;
const RIGHT = NODE_W - 8;
const WIDTH = RIGHT - LEFT;

/** Une carte : cadre, bande de statut, puis selon l'échelle tout le texte, le nom seul, ou rien. */
const TopoNode = memo(function TopoNode({
  node,
  x,
  y,
  focused,
  hovered,
  dim,
  pinned,
  textMode,
  nameSize,
}: {
  node: GraphNode;
  x: number;
  y: number;
  focused: boolean;
  hovered: boolean;
  dim: boolean;
  pinned: boolean;
  textMode: TextMode;
  nameSize: number;
}) {
  const tone: Tone = node.known ? statusTone(node.status) : "neutral";
  const cls =
    "topo-node" +
    (focused ? " focused" : "") +
    (hovered ? " hovered" : "") +
    (dim ? " dim" : "") +
    (node.known ? "" : " unknown");
  const family = node.namespace ? `${KIND_LABEL[node.kind]} · ${node.namespace}` : KIND_LABEL[node.kind];
  const readyText = node.ready ? fitText(node.ready, WIDTH * 0.4, SMALL) : "";
  const readyW = readyText ? readyText.length * SMALL * 0.56 + 6 : 0;
  const statusText = fitText(node.known ? node.status || "–" : "non listé", WIDTH * 0.45, SMALL);
  const statusW = statusText.length * SMALL * 0.56 + 6;
  return (
    <g className={cls} transform={`translate(${r1(x)} ${r1(y)})`} data-key={node.key}>
      <rect className="topo-card" width={NODE_W} height={NODE_H} rx={6} />
      <rect className={`topo-stripe tone-${tone}`} x={1} y={1} width={4} height={NODE_H - 2} rx={2} />
      {textMode === "full" && (
        <>
          <text className="topo-family" x={LEFT} y={7} dominantBaseline="hanging">
            {fitText(family, WIDTH - readyW, SMALL)}
          </text>
          {readyText && (
            <text className="topo-ready" x={RIGHT} y={7} textAnchor="end" dominantBaseline="hanging">
              {readyText}
            </text>
          )}
          <text className="topo-name" x={LEFT} y={NODE_H - 8}>
            {fitText(node.name, WIDTH - statusW, BODY)}
          </text>
          <text className={`topo-status tone-${tone}`} x={RIGHT} y={NODE_H - 8} textAnchor="end">
            {statusText}
          </text>
        </>
      )}
      {textMode === "name" && (
        <text className="topo-name" x={LEFT} y={NODE_H / 2} dominantBaseline="central" fontSize={nameSize}>
          {fitText(node.name, WIDTH, nameSize)}
        </text>
      )}
      {pinned && <circle className="topo-pin" cx={NODE_W - 1} cy={1} r={4} />}
    </g>
  );
});

/** Infobulle d'une carte survolée. */
function Tooltip({
  node,
  x,
  top,
  bottom,
  width,
  height,
}: {
  node: GraphNode;
  x: number;
  top: number;
  bottom: number;
  width: number;
  height: number;
}) {
  const tone = statusTone(node.status);
  const left = Math.max(8, Math.min(x, width - 348));
  // Sous la carte, ou au-dessus quand la place manque en bas.
  const flip = bottom + 170 > height && top > height - bottom;
  const style = flip ? { left, bottom: Math.max(8, height - top + 6) } : { left, top: Math.max(8, bottom + 6) };
  return (
    <div className="topo-tip" style={style}>
      <div className="topo-tip-title selectable">{node.name}</div>
      <div className="muted">{node.namespace ? `${node.kind} · namespace ${node.namespace}` : `${node.kind} · portée cluster`}</div>
      {!node.known ? (
        <div className="topo-tip-warn">
          Référencé par un autre objet, mais non listé : couche masquée, objet absent ou droits insuffisants.
        </div>
      ) : (
        <div className="topo-kv">
          <span className="muted">Statut</span>
          <span className={`tone-${tone}`}>{node.status || "–"}</span>
          {node.ready && (
            <>
              <span className="muted">Prêt</span>
              <span>{node.ready}</span>
            </>
          )}
          {node.restarts != null && node.restarts > 0 && (
            <>
              <span className="muted">Redémarrages</span>
              <span className="tone-warn">{node.restarts}</span>
            </>
          )}
          {node.node && (
            <>
              <span className="muted">Nœud</span>
              <span>{node.node}</span>
            </>
          )}
          {node.ageSeconds != null && (
            <>
              <span className="muted">Âge</span>
              <span>{age(node.ageSeconds)}</span>
            </>
          )}
          {node.images.slice(0, 3).map((img, i) => (
            <span key={img + i} style={{ display: "contents" }}>
              <span className="muted">{i === 0 ? "Images" : ""}</span>
              <span className="mono truncate">{img}</span>
            </span>
          ))}
          {node.images.length > 3 && (
            <>
              <span />
              <span className="muted">… et {count(node.images.length - 3, "autre")}</span>
            </>
          )}
        </div>
      )}
      <div className="faint">clic : détail · glisser : déplacer · clic droit : menu</div>
    </div>
  );
}

/** Panneau de détail léger : identité, statut, images, liens. */
function DetailPanel({
  node,
  graph,
  pinned,
  onClose,
  onOpen,
  onCenter,
  onTogglePin,
  onSelect,
}: {
  node: GraphNode;
  graph: Graph;
  pinned: boolean;
  onClose: () => void;
  onOpen: () => void;
  onCenter: () => void;
  onTogglePin: () => void;
  onSelect: (key: string) => void;
}) {
  const i = graph.index.get(node.key) ?? -1;
  const links = useMemo(() => {
    const out: { label: string; other: GraphNode; dir: "out" | "in" }[] = [];
    for (const e of graph.edges) {
      if (e.from === i) {
        const other = graph.nodes[e.to];
        if (other) out.push({ label: EDGE_LABEL[e.kind], other, dir: "out" });
      } else if (e.to === i) {
        const other = graph.nodes[e.from];
        if (other) out.push({ label: EDGE_LABEL_REVERSE[e.kind], other, dir: "in" });
      }
    }
    out.sort((a, b) => a.dir.localeCompare(b.dir) || a.label.localeCompare(b.label) || a.other.name.localeCompare(b.other.name));
    return out;
  }, [graph, i]);
  const row = (k: string, v: React.ReactNode) => (
    <div className="topo-kv-row" key={k}>
      <span className="muted xs">{k}</span>
      <span className="selectable">{v}</span>
    </div>
  );
  return (
    <aside className="topo-detail">
      <div className="toolbar" style={{ gap: 6 }}>
        <Badge tone="info">{node.kind}</Badge>
        <span className="row gap-4 grow truncate" style={{ minWidth: 0 }}>
          {node.namespace && <span className="muted">{node.namespace} /</span>}
          <strong className="truncate selectable" title={node.name}>
            {node.name}
          </strong>
        </span>
        <button className="btn btn-ghost btn-sm btn-icon" onClick={onClose} title="Fermer (Échap)">
          <X size={14} />
        </button>
      </div>
      <div className="scroll p-16 col gap-12">
        {!node.known && (
          <Alert tone="warn">
            Référencé par un autre objet, mais non listé : couche masquée, objet absent ou droits insuffisants.
          </Alert>
        )}
        <div className="row wrap">
          <button className="btn btn-primary btn-sm" onClick={onOpen}>
            <ArrowSquareOut size={14} /> Ouvrir dans Ressources
          </button>
          <button className="btn btn-sm" onClick={onCenter} title="Centrer la vue sur cet objet">
            <Crosshair size={14} /> Centrer
          </button>
          <button className="btn btn-sm" onClick={onTogglePin} title={pinned ? "Laisser la disposition le replacer" : "Ne plus le déplacer automatiquement"}>
            {pinned ? <PushPinSlash size={14} /> : <PushPin size={14} />} {pinned ? "Détacher" : "Épingler"}
          </button>
        </div>
        <div className="topo-kv-list">
          {row("Nom", node.name)}
          {row("Type", node.apiVersion ? `${node.kind} (${node.apiVersion})` : node.kind)}
          {row("Namespace", node.namespace ?? <span className="muted">portée cluster</span>)}
          {node.known && row("Statut", <StatusBadge status={node.status} />)}
          {node.ready && row("Prêt", <ReadyBadge ready={node.ready} />)}
          {node.restarts != null && node.restarts > 0 && row("Redémarrages", node.restarts)}
          {node.node && row("Nœud", node.node)}
          {node.ageSeconds != null && row("Âge", age(node.ageSeconds))}
        </div>
        {node.images.length > 0 && (
          <div className="col gap-4">
            <div className="topo-section-title">Images</div>
            {node.images.map((img) => (
              <span key={img} className="mono xs selectable" title={img} style={{ wordBreak: "break-all" }}>
                {img}
              </span>
            ))}
          </div>
        )}
        <div className="col gap-4">
          <div className="topo-section-title">Liens ({links.length})</div>
          {links.length === 0 && <span className="muted small">Aucun lien avec les couches affichées.</span>}
          {links.map((l, idx) => (
            <button key={idx} className="topo-link" onClick={() => onSelect(l.other.key)} title={`${l.label} ${l.other.kind} ${l.other.name}`}>
              <span className="muted xs nowrap">{l.label}</span>
              <Badge>{KIND_LABEL[l.other.kind]}</Badge>
              <span className="truncate">{l.other.name}</span>
            </button>
          ))}
        </div>
      </div>
    </aside>
  );
}
