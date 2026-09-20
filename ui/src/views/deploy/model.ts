// Modèle de l'assistant de déploiement : étapes, profils de taille, modes
// d'accès, état simplifié et sa traduction en DeployRequest.
// Miroir de crates/desktop/src/views/deploy.rs (WizardState, Exposure,
// prefill_*) et de crates/hub/src/sizing.rs (SizeProfile, recommended_profile).
import type { CatalogApp, DeployRequest, PortSpec } from "@/api/hub";

// ---------------------------------------------------------------------------
// Étapes
// ---------------------------------------------------------------------------

export type Step = "what" | "how" | "preview" | "result";

export const STEPS: { id: Step; label: string }[] = [
  { id: "what", label: "Quoi" },
  { id: "how", label: "Comment" },
  { id: "preview", label: "Vérification" },
  { id: "result", label: "Résultat" },
];

export function stepIndex(step: Step): number {
  return STEPS.findIndex((s) => s.id === step);
}

/** La plus avancée des deux étapes. */
export function furthest(a: Step, b: Step): Step {
  return stepIndex(a) >= stepIndex(b) ? a : b;
}

export type Source = "catalog" | "image";

// ---------------------------------------------------------------------------
// Modes d'accès
// ---------------------------------------------------------------------------

export type Exposure = "internal" | "nodePort" | "loadBalancer" | "domain";

export interface ExposureInfo {
  id: Exposure;
  label: string;
  description: string;
  serviceType: "ClusterIP" | "NodePort" | "LoadBalancer";
}

export const EXPOSURE_ORDER: Exposure[] = ["internal", "nodePort", "loadBalancer", "domain"];

export const EXPOSURES: Record<Exposure, ExposureInfo> = {
  internal: {
    id: "internal",
    label: "Interne au cluster",
    description: "Les autres applications du cluster y accèdent par son nom. Rien n'est ouvert vers l'extérieur.",
    serviceType: "ClusterIP",
  },
  nodePort: {
    id: "nodePort",
    label: "Port sur les nœuds",
    description: "Kubernetes ouvre le même port sur tous les nœuds. Pratique en local, rarement en production.",
    serviceType: "NodePort",
  },
  loadBalancer: {
    id: "loadBalancer",
    label: "Adresse IP publique",
    description: "L'hébergeur attribue une adresse IP. Sans contrôleur adapté, le Service reste « pending ».",
    serviceType: "LoadBalancer",
  },
  domain: {
    id: "domain",
    label: "Nom de domaine",
    description: "Un Ingress route un nom de domaine vers l'application. Il faut un contrôleur d'entrée sur le cluster.",
    serviceType: "ClusterIP",
  },
};

// ---------------------------------------------------------------------------
// Profils de taille
// ---------------------------------------------------------------------------

export type SizeProfile = "micro" | "small" | "medium" | "large";

export interface ProfileInfo {
  id: SizeProfile;
  label: string;
  description: string;
  cpuRequest: string;
  cpuLimit: string;
  memoryRequest: string;
  memoryLimit: string;
}

export const PROFILE_ORDER: SizeProfile[] = ["micro", "small", "medium", "large"];

export const PROFILES: Record<SizeProfile, ProfileInfo> = {
  micro: {
    id: "micro",
    label: "Micro",
    description: "Page statique, outil d'appoint, démonstration.",
    cpuRequest: "10m",
    cpuLimit: "200m",
    memoryRequest: "32Mi",
    memoryLimit: "128Mi",
  },
  small: {
    id: "small",
    label: "Petit",
    description: "API légère, service métier peu sollicité.",
    cpuRequest: "50m",
    cpuLimit: "500m",
    memoryRequest: "128Mi",
    memoryLimit: "512Mi",
  },
  medium: {
    id: "medium",
    label: "Moyen",
    description: "Base de données modeste, service au trafic régulier.",
    cpuRequest: "250m",
    cpuLimit: "1",
    memoryRequest: "512Mi",
    memoryLimit: "2Gi",
  },
  large: {
    id: "large",
    label: "Grand",
    description: "Base de données chargée, traitement gourmand.",
    cpuRequest: "1",
    cpuLimit: "2",
    memoryRequest: "2Gi",
    memoryLimit: "4Gi",
  },
};

/** Profil conseillé pour une application du catalogue : la catégorie est le seul indice. */
export function recommendedProfile(app: CatalogApp): SizeProfile {
  switch (app.category) {
    case "Base de données":
    case "Observabilité":
    case "Stockage":
    case "Automatisation":
    case "Gestion de code":
      return "medium";
    case "Cache":
    case "File d'attente":
      return "small";
    default:
      return app.needsPvc ? "medium" : "small";
  }
}

// ---------------------------------------------------------------------------
// État de l'assistant
// ---------------------------------------------------------------------------

/**
 * Tout ce que l'utilisateur manipule. `buildRequest` en tire la DeployRequest
 * envoyée au backend ; les champs que l'assistant n'expose pas (commande,
 * arguments, placement…) gardent leur valeur par défaut côté Rust.
 */
export interface Wizard {
  step: Step;
  /** Étape la plus avancée atteinte, qui borne le fil cliquable. */
  reached: Step;
  source: Source;
  /** Filtre textuel de la grille du catalogue. */
  filter: string;
  /** Catégorie retenue ; `null` = toutes. */
  category: string | null;
  /** Identifiant de l'application choisie dans le catalogue. */
  picked: string | null;
  /** Référence d'image saisie à la main. */
  imageInput: string;

  name: string;
  namespace: string;
  image: string;
  replicas: number;
  ports: PortSpec[];
  env: [string, string][];
  labels: [string, string][];

  profile: SizeProfile;
  exposure: Exposure;
  host: string;
  ingressClass: string;
  ingressTlsSecret: string;

  storage: boolean;
  storageGib: number;
  storagePath: string;
  storageClass: string;
}

export function newWizard(namespace: string): Wizard {
  return {
    step: "what",
    reached: "what",
    source: "catalog",
    filter: "",
    category: null,
    picked: null,
    imageInput: "",
    name: "",
    namespace,
    image: "",
    replicas: 1,
    ports: [],
    env: [],
    labels: [],
    profile: "small",
    exposure: "internal",
    host: "",
    ingressClass: "",
    ingressTlsSecret: "",
    storage: false,
    storageGib: 10,
    storagePath: "/data",
    storageClass: "",
  };
}

/** Revient à la première étape et oublie l'application choisie ; le namespace survit. */
export function restart(w: Wizard): Wizard {
  return newWizard(w.namespace);
}

/** Prépare l'assistant pour une application du catalogue. */
export function prefillFromCatalog(w: Wizard, app: CatalogApp): Wizard {
  return {
    ...newWizard(w.namespace),
    source: "catalog",
    filter: w.filter,
    category: w.category,
    picked: app.id,
    imageInput: app.image,
    name: sanitizeName(app.id),
    image: app.image,
    replicas: 1,
    ports: app.defaultPort != null ? [{ containerPort: app.defaultPort }] : [],
    env: app.env.map(([k, v]) => [k, v]),
    profile: recommendedProfile(app),
    storage: app.needsPvc,
  };
}

/** Prépare l'assistant pour une image saisie à la main. */
export function prefillFromImage(w: Wizard, reference: string, ports: number[]): Wizard {
  const image = reference.trim();
  return {
    ...newWizard(w.namespace),
    source: "image",
    filter: w.filter,
    category: w.category,
    picked: null,
    imageInput: image,
    name: sanitizeName(shortName(image)),
    image,
    replicas: 1,
    ports: ports.map((p) => ({ containerPort: p })),
    profile: "small",
  };
}

// ---------------------------------------------------------------------------
// Brouillon venu de l'écran Hub
// ---------------------------------------------------------------------------

export const DRAFT_KEY = "kubewatch.deployDraft";

export interface DeployDraft {
  image: string;
  name?: string;
  ports?: number[];
  env?: [string, string][];
  catalogAppId?: string;
}

/** Lit puis efface le brouillon déposé par l'écran Hub ; `null` s'il n'y en a pas ou s'il est illisible. */
export function readDraft(): DeployDraft | null {
  let raw: string | null = null;
  try {
    raw = sessionStorage.getItem(DRAFT_KEY);
    if (raw != null) sessionStorage.removeItem(DRAFT_KEY);
  } catch {
    return null;
  }
  if (!raw) return null;
  try {
    const value = JSON.parse(raw) as unknown;
    if (!value || typeof value !== "object") return null;
    const o = value as Record<string, unknown>;
    if (typeof o.image !== "string" || !o.image.trim()) return null;
    const draft: DeployDraft = { image: o.image };
    if (typeof o.name === "string") draft.name = o.name;
    if (typeof o.catalogAppId === "string") draft.catalogAppId = o.catalogAppId;
    if (Array.isArray(o.ports)) {
      draft.ports = o.ports.filter((p): p is number => typeof p === "number" && Number.isInteger(p) && p >= 1 && p <= 65535);
    }
    if (Array.isArray(o.env)) {
      draft.env = o.env.filter(
        (e): e is [string, string] => Array.isArray(e) && e.length === 2 && typeof e[0] === "string" && typeof e[1] === "string",
      );
    }
    return draft;
  } catch {
    return null;
  }
}

/** Applique un brouillon : pré-remplissage depuis le catalogue si l'application est connue, sinon depuis l'image. */
export function applyDraft(w: Wizard, draft: DeployDraft, app: CatalogApp | null): Wizard {
  const base = app ? prefillFromCatalog(w, app) : prefillFromImage(w, draft.image, draft.ports ?? []);
  const image = draft.image.trim() || base.image;
  return {
    ...base,
    image,
    imageInput: image,
    name: draft.name?.trim() ? sanitizeName(draft.name) : base.name,
    ports: draft.ports?.length ? draft.ports.map((p) => ({ containerPort: p })) : base.ports,
    env: draft.env?.length ? draft.env.map(([k, v]) => [k, v] as [string, string]) : base.env,
    step: "how",
    reached: furthest(base.reached, "how"),
  };
}

// ---------------------------------------------------------------------------
// Vers la requête
// ---------------------------------------------------------------------------

/** La requête nettoyée, telle qu'elle sera envoyée au backend. */
export function buildRequest(w: Wizard): DeployRequest {
  const profile = PROFILES[w.profile];
  const exposure = EXPOSURES[w.exposure];
  const domain = w.exposure === "domain";
  const optional = (v: string): string | null => (v.trim() ? v.trim() : null);
  return {
    name: w.name.trim(),
    namespace: w.namespace.trim() || "default",
    image: w.image.trim(),
    replicas: w.replicas,
    ports: w.ports.filter((p) => p.containerPort !== 0).map((p) => ({ ...p })),
    env: w.env.filter(([k]) => k.trim()).map(([k, v]) => [k.trim(), v] as [string, string]),
    cpuRequest: profile.cpuRequest,
    cpuLimit: profile.cpuLimit,
    memoryRequest: profile.memoryRequest,
    memoryLimit: profile.memoryLimit,
    serviceType: exposure.serviceType,
    ingressHost: domain ? optional(w.host) : null,
    ingressClass: domain ? optional(w.ingressClass) : null,
    ingressTlsSecret: domain ? optional(w.ingressTlsSecret) : null,
    pvc: w.storage
      ? {
          size: `${Math.max(1, Math.trunc(w.storageGib) || 1)}Gi`,
          mountPath: w.storagePath.trim() || "/data",
          storageClass: optional(w.storageClass),
        }
      : null,
    labels: Object.fromEntries(w.labels.filter(([k]) => k.trim()).map(([k, v]) => [k.trim(), v])),
  };
}

// ---------------------------------------------------------------------------
// Noms
// ---------------------------------------------------------------------------

/** `ghcr.io/o/depot:1.2` → `depot`. */
export function shortName(reference: string): string {
  const noDigest = reference.split("@")[0] ?? reference;
  const last = noDigest.split("/").pop() ?? noDigest;
  return last.split(":")[0] ?? last;
}

/** Ramène un texte quelconque sur une étiquette DNS-1123 utilisable comme nom. */
export function sanitizeName(raw: string): string {
  let out = "";
  for (const c of raw.trim().toLowerCase()) {
    if (/^[a-z0-9]$/.test(c)) out += c;
    else if (!out.endsWith("-")) out += "-";
  }
  out = out.replace(/^-+|-+$/g, "").slice(0, 63).replace(/-+$/, "");
  return out || "application";
}
