// Prévision de l'empreinte d'un déploiement, confrontée à la capacité connue
// du cluster. Miroir de crates/hub/src/sizing.rs (footprint, assess, notes) et
// de crates/core/src/metrics.rs (analyse des quantités Kubernetes).
//
// L'empreinte additionnée est celle des demandes (`resources.requests`), car
// c'est sur elles que l'ordonnanceur réserve la place ; le point de départ est
// la consommation mesurée du cluster (metrics.k8s.io). C'est un ordre de
// grandeur honnête, pas une simulation d'ordonnanceur.
import type { DeployRequest } from "@/api/hub";
import type { ClusterOverview } from "@/api/types";

/** Seuil au-delà duquel l'occupation prévue est jugée serrée. */
const TIGHT_FRACTION = 0.85;
const GIB = 1024 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Quantités Kubernetes
// ---------------------------------------------------------------------------

const FACTORS: Record<string, number> = {
  n: 1e-9,
  u: 1e-6,
  "µ": 1e-6,
  m: 1e-3,
  k: 1e3,
  K: 1e3,
  M: 1e6,
  G: 1e9,
  T: 1e12,
  P: 1e15,
  E: 1e18,
  Ki: 1024,
  Mi: 1024 ** 2,
  Gi: 1024 ** 3,
  Ti: 1024 ** 4,
  Pi: 1024 ** 5,
  Ei: 1024 ** 6,
};

const isDigit = (c: string | undefined): boolean => c != null && c >= "0" && c <= "9";

/** Analyse une quantité Kubernetes (`100m`, `2Gi`, `1e3`) en nombre sans unité. */
export function parseQuantity(q: string): number | null {
  const text = q.trim();
  if (!text) return null;
  let i = 0;
  if (text[i] === "+" || text[i] === "-") i++;
  const intStart = i;
  while (isDigit(text[i])) i++;
  const intLen = i - intStart;
  let fracLen = 0;
  if (text[i] === ".") {
    i++;
    const fracStart = i;
    while (isDigit(text[i])) i++;
    fracLen = i - fracStart;
  }
  if (intLen === 0 && fracLen === 0) return null;
  const mantissa = Number(text.slice(0, i));
  if (!Number.isFinite(mantissa)) return null;
  const suffix = text.slice(i);
  if (!suffix) return mantissa;
  // Exposant décimal : `e`/`E` suivi d'un entier signé. Un `E` seul est le suffixe « exa ».
  if (suffix[0] === "e" || suffix[0] === "E") {
    const rest = suffix.slice(1);
    if (/^[+-]?[0-9]+$/.test(rest)) return mantissa * 10 ** Number(rest);
  }
  const factor = FACTORS[suffix];
  if (factor === undefined) return null;
  return mantissa * factor;
}

/** Quantité CPU en millicœurs (`100m` → 100, `1` → 1000). */
export function parseCpuMillis(q: string): number | null {
  const cores = parseQuantity(q);
  return cores == null || !Number.isFinite(cores) ? null : cores * 1000;
}

/** Quantité mémoire en octets (`2Gi` → 2147483648). */
export function parseMemoryBytes(q: string): number | null {
  const b = parseQuantity(q);
  return b == null || !Number.isFinite(b) ? null : Math.round(b);
}

/** Taille en gibioctets d'une quantité, arrondie au plus proche (au moins 1). */
export function gibOf(quantity: string): number | null {
  const b = parseMemoryBytes(quantity);
  if (b == null || b <= 0) return null;
  return Math.max(1, Math.round(b / GIB));
}

// ---------------------------------------------------------------------------
// Empreinte
// ---------------------------------------------------------------------------

/** Ce qu'un déploiement va réserver ; CPU et mémoire déjà multipliés par les répliques. */
export interface Footprint {
  replicas: number;
  cpuRequestMillis: number | null;
  cpuLimitMillis: number | null;
  memoryRequestBytes: number | null;
  memoryLimitBytes: number | null;
  /** Un seul PVC, monté par tous les pods : pas multiplié. */
  storageBytes: number | null;
}

const cpuOf = (q: string | null | undefined, factor: number): number | null => {
  const v = q?.trim();
  if (!v) return null;
  const m = parseCpuMillis(v);
  return m == null ? null : m * factor;
};
const memoryOf = (q: string | null | undefined, factor: number): number | null => {
  const v = q?.trim();
  if (!v) return null;
  const b = parseMemoryBytes(v);
  return b == null ? null : b * factor;
};

export function footprint(req: DeployRequest): Footprint {
  const replicas = Math.max(0, req.replicas ?? 1);
  return {
    replicas,
    cpuRequestMillis: cpuOf(req.cpuRequest, replicas),
    cpuLimitMillis: cpuOf(req.cpuLimit, replicas),
    memoryRequestBytes: memoryOf(req.memoryRequest, replicas),
    memoryLimitBytes: memoryOf(req.memoryLimit, replicas),
    storageBytes: req.pvc ? parseMemoryBytes(req.pvc.size) : null,
  };
}

// ---------------------------------------------------------------------------
// Confrontation à la capacité du cluster
// ---------------------------------------------------------------------------

/** Un axe de capacité (CPU ou mémoire) avant et après le déploiement. */
export interface Axis {
  used: number;
  added: number;
  capacity: number;
}

function fraction(value: number, total: number): number {
  if (total <= 0 || !Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(1, value / total));
}

export const beforeFraction = (a: Axis): number => fraction(a.used, a.capacity);
export const afterFraction = (a: Axis): number => fraction(a.used + a.added, a.capacity);
/** Part occupée après, sans borne : au-delà de 1, ça déborde. */
export const afterRatio = (a: Axis): number => (a.capacity <= 0 ? 0 : (a.used + a.added) / a.capacity);
/** Ce qui reste libre après ; négatif en cas de dépassement. */
export const remaining = (a: Axis): number => a.capacity - a.used - a.added;

export type Fit = "fits" | "tight" | "exceeds" | "unknown";

export const FIT_HEADLINE: Record<Fit, string> = {
  fits: "Ce déploiement tient sans difficulté.",
  tight: "Ce déploiement passe, mais le cluster sera chargé.",
  exceeds: "Ce déploiement dépasse la capacité du cluster.",
  unknown: "Consommation du cluster inconnue : la prévision est incomplète.",
};

export type NoteLevel = "info" | "warning" | "danger";
const LEVEL_RANK: Record<NoteLevel, number> = { info: 0, warning: 1, danger: 2 };

export interface Note {
  level: NoteLevel;
  text: string;
}

export interface Assessment {
  footprint: Footprint;
  cpu: Axis | null;
  memory: Axis | null;
  fit: Fit;
  /** Les plus graves en tête. */
  notes: Note[];
}

/** Confronte une requête à l'état connu du cluster ; sans synthèse, seules les remarques hors cluster subsistent. */
export function assess(req: DeployRequest, overview: ClusterOverview | null | undefined): Assessment {
  const fp = footprint(req);
  const measurable = overview && overview.metricsAvailable ? overview : null;
  const cpu: Axis | null =
    measurable && measurable.cpuCapacityMillis > 0
      ? { used: measurable.cpuUsedMillis, added: fp.cpuRequestMillis ?? 0, capacity: measurable.cpuCapacityMillis }
      : null;
  const memory: Axis | null =
    measurable && measurable.memoryCapacityBytes > 0
      ? { used: measurable.memoryUsedBytes, added: fp.memoryRequestBytes ?? 0, capacity: measurable.memoryCapacityBytes }
      : null;

  let fit: Fit = "unknown";
  const axes = [cpu, memory].filter((a): a is Axis => a != null);
  if (axes.length > 0) {
    const worst = Math.max(0, ...axes.map(afterRatio));
    fit = worst > 1 ? "exceeds" : worst > TIGHT_FRACTION ? "tight" : "fits";
  }

  const notes = collectNotes(req, fp, cpu, memory);
  notes.sort((a, b) => LEVEL_RANK[b.level] - LEVEL_RANK[a.level]);
  return { footprint: fp, cpu, memory, fit, notes };
}

function collectNotes(req: DeployRequest, fp: Footprint, cpu: Axis | null, memory: Axis | null): Note[] {
  const notes: Note[] = [];
  const note = (level: NoteLevel, text: string) => notes.push({ level, text });

  // --- dépassement de capacité
  if (cpu && afterRatio(cpu) > 1) {
    note("danger", `Il manque ${Math.round(-remaining(cpu))} m de CPU : les pods resteront en attente d'un nœud capable de les accueillir.`);
  }
  if (memory && afterRatio(memory) > 1) {
    note("danger", `Il manque ${humanBytes(-remaining(memory))} de mémoire : les pods resteront en attente d'un nœud capable de les accueillir.`);
  }

  // --- demandes de ressources absentes
  if (fp.cpuRequestMillis == null || fp.memoryRequestBytes == null) {
    note(
      "warning",
      "Sans demande de ressources, l'ordonnanceur place les pods à l'aveugle et ils seront les premiers arrêtés quand un nœud manquera de mémoire.",
    );
  }

  // --- limite inférieure à la demande : Kubernetes refuse l'objet
  const cpuR = req.cpuRequest ? parseCpuMillis(req.cpuRequest) : null;
  const cpuL = req.cpuLimit ? parseCpuMillis(req.cpuLimit) : null;
  if (cpuR != null && cpuL != null && cpuL < cpuR) {
    note("danger", "La limite CPU est inférieure à la demande : Kubernetes refusera le déploiement.");
  }
  const memR = req.memoryRequest ? parseMemoryBytes(req.memoryRequest) : null;
  const memL = req.memoryLimit ? parseMemoryBytes(req.memoryLimit) : null;
  if (memR != null && memL != null && memL < memR) {
    note("danger", "La limite mémoire est inférieure à la demande : Kubernetes refusera le déploiement.");
  }

  // --- volume persistant partagé par plusieurs répliques
  const replicas = req.replicas ?? 1;
  if (req.pvc) {
    const mode = req.pvc.accessMode?.trim() || "ReadWriteOnce";
    if (replicas > 1 && (mode === "ReadWriteOnce" || mode === "ReadWriteOncePod")) {
      note("danger", `Le volume est en ${mode} : une seule réplique pourra le monter, les ${replicas - 1} autres resteront bloquées au démarrage.`);
    }
  }

  // --- nombre de répliques
  if (replicas === 0) {
    note("info", "Zéro réplique : les objets seront créés, mais aucun pod ne démarrera.");
  } else if (replicas === 1) {
    note("info", "Une seule réplique : l'application sera interrompue pendant les redémarrages et les mises à jour.");
  }

  // --- tag mouvant
  const image = req.image.trim();
  if (image.endsWith(":latest") || (!image.includes(":") && !image.includes("@"))) {
    note("warning", "L'image vise « latest » : le contenu déployé changera sans prévenir. Épinglez une version pour garder un déploiement reproductible.");
  }

  // --- exposition
  if ((req.ports ?? []).length === 0) {
    note("info", "Aucun port déclaré : aucun Service ne sera créé et l'application ne sera joignable que depuis son propre pod.");
  }
  if (req.ingressHost?.trim() && !req.ingressClass?.trim()) {
    note("info", "Aucune classe d'Ingress précisée : le cluster utilisera sa classe par défaut, s'il en a une.");
  }

  // --- cluster serré sans dépassement
  const tight = [cpu, memory].some((a) => a != null && afterRatio(a) > TIGHT_FRACTION && afterRatio(a) <= 1);
  if (tight) {
    note("warning", "Le cluster dépassera 85 % d'occupation : il ne restera guère de marge pour absorber un pic ou la perte d'un nœud.");
  }

  return notes;
}

/** Taille lisible en octets binaires, pour les textes de remarque. */
function humanBytes(bytes: number): string {
  const units = ["o", "Kio", "Mio", "Gio", "Tio"];
  let value = Math.abs(bytes);
  let i = 0;
  while (value >= 1024 && i + 1 < units.length) {
    value /= 1024;
    i++;
  }
  return i === 0 ? `${value.toFixed(0)} o` : `${value.toFixed(1)} ${units[i]}`;
}
