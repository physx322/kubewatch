// Validation d'une DeployRequest côté interface, règle pour règle avec
// crates/hub/src/deploy.rs (validate) et crates/hub/src/model.rs (ImageRef::parse).
// Le backend reste juge en dernier ressort ; ici on évite un aller-retour et on
// signale le champ fautif avant l'étape de vérification.
import type { DeployRequest, PortSpec, PvcSpec } from "@/api/hub";

const DNS1123_LABEL = /^[a-z0-9]([-a-z0-9]*[a-z0-9])?$/;
const PORT_NAME = /^[a-z0-9]([-a-z0-9]*[a-z0-9])?$/;
const QUANTITY = /^(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?(?:m|k|M|G|T|P|E|Ki|Mi|Gi|Ti|Pi|Ei)?$/;
const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;
const HOSTNAME = /^(?:\*\.)?(?:[a-zA-Z0-9](?:[-a-zA-Z0-9]*[a-zA-Z0-9])?\.)*[a-zA-Z0-9](?:[-a-zA-Z0-9]*[a-zA-Z0-9])?$/;
const LABEL_KEY_NAME = /^[A-Za-z0-9]([-A-Za-z0-9_.]*[A-Za-z0-9])?$/;

const SERVICE_TYPES = ["ClusterIP", "NodePort", "LoadBalancer"];
const ACCESS_MODES = ["ReadWriteOnce", "ReadOnlyMany", "ReadWriteMany", "ReadWriteOncePod"];

/** Étiquette DNS-1123 valide (63 caractères au plus). */
export function isDns1123Label(value: string): boolean {
  return value.length > 0 && value.length <= 63 && DNS1123_LABEL.test(value);
}

/** Quantité Kubernetes strictement positive (`100m`, `0.5`, `256Mi`, `2Gi`). */
export function isQuantity(value: string): boolean {
  const v = value.trim();
  if (!v || !QUANTITY.test(v)) return false;
  const digits = /^[0-9.]*/.exec(v)?.[0] ?? "";
  const n = Number(digits);
  return Number.isFinite(n) && n > 0;
}

// ---------------------------------------------------------------------------
// Références d'image
// ---------------------------------------------------------------------------

const DEFAULT_REGISTRY = "docker.io";
const DOCKER_OFFICIAL_NAMESPACE = "library";
const DEFAULT_TAG = "latest";
const REPOSITORY_SEGMENT = /^[a-zA-Z0-9]([a-zA-Z0-9._-]*[a-zA-Z0-9])?$/;
const TAG_PATTERN = /^[A-Za-z0-9_][A-Za-z0-9._-]{0,127}$/;
const DIGEST_PATTERN = /^[a-z0-9]+(?:[.+_-][a-z0-9]+)*:[a-fA-F0-9]{32,}$/;

export type ImageRefResult =
  | { ok: true; registry: string; repository: string; tag: string | null; digest: string | null; full: string }
  | { ok: false; error: string };

/** Analyse une référence d'image sous toutes ses formes usuelles et rend sa forme canonique. */
export function parseImageRef(s: string): ImageRefResult {
  const raw = s.trim();
  if (!raw) return { ok: false, error: "référence d'image vide : indiquez par exemple « nginx:1.27 »" };
  if (/\s/.test(raw)) return { ok: false, error: `référence d'image « ${raw} » invalide : elle ne doit pas contenir d'espace` };

  // 1. Détacher le digest éventuel.
  let rest = raw;
  let digest: string | null = null;
  const at = raw.indexOf("@");
  if (at >= 0) {
    const tail = raw.slice(at + 1);
    if (!DIGEST_PATTERN.test(tail)) return { ok: false, error: `digest « ${tail} » invalide : attendu « sha256:<hexadécimal> »` };
    rest = raw.slice(0, at);
    digest = tail.toLowerCase();
  }
  if (!rest) return { ok: false, error: `référence « ${raw} » invalide : le nom du dépôt est manquant avant le digest` };

  // 2. Détacher l'hôte du registre : le premier segment n'en est un que s'il ressemble à un nom d'hôte.
  let registry = DEFAULT_REGISTRY;
  let remainder = rest;
  const slash = rest.indexOf("/");
  if (slash >= 0) {
    const head = rest.slice(0, slash);
    const tail = rest.slice(slash + 1);
    if (head === "localhost" || head.includes(".") || head.includes(":")) {
      if (!tail) return { ok: false, error: `référence « ${raw} » invalide : nom de dépôt manquant après « ${head} »` };
      registry = normalizeRegistry(head.toLowerCase());
      remainder = tail;
    }
  }

  // 3. Détacher le tag : dans ce qui reste, un « : » ne peut plus être un port.
  let repository = remainder;
  let tag: string | null = null;
  const colon = remainder.lastIndexOf(":");
  if (colon >= 0 && !remainder.slice(colon + 1).includes("/")) {
    const t = remainder.slice(colon + 1);
    if (!t) return { ok: false, error: `référence « ${raw} » invalide : tag vide après « : »` };
    if (!TAG_PATTERN.test(t)) return { ok: false, error: `tag « ${t} » invalide` };
    repository = remainder.slice(0, colon);
    tag = t;
  }
  if (!repository) return { ok: false, error: `référence « ${raw} » invalide : nom de dépôt manquant` };
  for (const segment of repository.split("/")) {
    if (!REPOSITORY_SEGMENT.test(segment)) {
      return { ok: false, error: `dépôt « ${repository} » invalide : le segment « ${segment} » n'est pas un nom valide` };
    }
  }

  // 4. Les images officielles Docker Hub vivent sous « library/ ».
  if (registry === DEFAULT_REGISTRY && !repository.includes("/")) repository = `${DOCKER_OFFICIAL_NAMESPACE}/${repository}`;

  // 5. Sans tag ni digest, on vise « latest ».
  if (tag == null && digest == null) tag = DEFAULT_TAG;

  let full = `${registry}/${repository}`;
  if (tag != null) full += `:${tag}`;
  if (digest != null) full += `@${digest}`;
  return { ok: true, registry, repository, tag, digest, full };
}

function normalizeRegistry(host: string): string {
  switch (host) {
    case "index.docker.io":
    case "registry-1.docker.io":
    case "registry.hub.docker.com":
      return DEFAULT_REGISTRY;
    default:
      return host;
  }
}

// ---------------------------------------------------------------------------
// Validation de la requête
// ---------------------------------------------------------------------------

function portProtocol(p: PortSpec): string {
  const v = p.protocol?.trim().toUpperCase();
  return v ? v : "TCP";
}

function portName(p: PortSpec): string {
  const n = p.name?.trim();
  return n ? n.toLowerCase() : `p${p.containerPort}`;
}

function pvcName(pvc: PvcSpec, app: string): string {
  const n = pvc.name?.trim();
  return n ? n : `${app}-data`;
}

function optionalLabel(field: string, value: string | null | undefined): string | null {
  const v = value?.trim();
  if (v && !isDns1123Label(v)) {
    return `${field} « ${v} » invalide : utilisez des minuscules, des chiffres et des tirets (63 caractères au plus).`;
  }
  return null;
}

function metadataKey(key: string): string | null {
  if (!key || key.length > 253) return `clé « ${key} » invalide : elle doit contenir entre 1 et 253 caractères.`;
  let name = key;
  const slash = key.indexOf("/");
  if (slash >= 0) {
    const prefix = key.slice(0, slash);
    if (!prefix || !HOSTNAME.test(prefix)) {
      return `préfixe de clé « ${prefix} » invalide : il doit être un nom DNS, par exemple « app.kubernetes.io ».`;
    }
    name = key.slice(slash + 1);
  }
  if (!name || name.length > 63 || !LABEL_KEY_NAME.test(name)) {
    return `clé « ${key} » invalide : la partie après « / » doit contenir au plus 63 caractères alphanumériques, tirets, points ou soulignés.`;
  }
  return null;
}

/**
 * Vérifie qu'une requête est applicable telle quelle. Rend le premier message
 * d'erreur (en français, désignant le champ fautif), ou `null` si tout va bien.
 */
export function validate(req: DeployRequest): string | null {
  if (!isDns1123Label(req.name)) {
    return `nom d'application « ${req.name} » invalide : utilisez uniquement des minuscules, des chiffres et des tirets, en commençant et finissant par un caractère alphanumérique (63 caractères au plus). Exemple : « mon-api ».`;
  }
  if (!isDns1123Label(req.namespace)) {
    return `namespace « ${req.namespace} » invalide : il doit respecter le format DNS-1123 (minuscules, chiffres et tirets). Exemple : « production ».`;
  }

  const image = req.image.trim();
  if (!image) return "image manquante : indiquez une référence complète, par exemple « nginx:1.27.3-alpine ».";
  const ref = parseImageRef(image);
  if (!ref.ok) return `image « ${image} » inutilisable : ${ref.error}`;

  const replicas = req.replicas ?? 1;
  if (!Number.isInteger(replicas) || replicas < 0 || replicas > 1000) {
    return `nombre de répliques invalide (${replicas}) : indiquez une valeur entre 0 et 1000.`;
  }

  // --- ports
  const seenContainer = new Set<number>();
  const seenService = new Set<number>();
  const seenNames = new Set<string>();
  for (const port of req.ports ?? []) {
    if (!Number.isInteger(port.containerPort) || port.containerPort < 1 || port.containerPort > 65535) {
      return "port conteneur manquant ou nul : indiquez une valeur entre 1 et 65535.";
    }
    if (seenContainer.has(port.containerPort)) {
      return `le port conteneur ${port.containerPort} est déclaré deux fois : chaque port doit être unique.`;
    }
    seenContainer.add(port.containerPort);

    if (port.servicePort != null && (!Number.isInteger(port.servicePort) || port.servicePort < 1 || port.servicePort > 65535)) {
      return `port de service nul pour le port conteneur ${port.containerPort} : indiquez une valeur entre 1 et 65535.`;
    }
    const servicePort = port.servicePort ?? port.containerPort;
    if (seenService.has(servicePort)) {
      return `le port de service ${servicePort} est déclaré deux fois : un Service ne peut pas exposer deux fois le même port.`;
    }
    seenService.add(servicePort);

    const protocol = portProtocol(port);
    if (!["TCP", "UDP", "SCTP"].includes(protocol)) {
      return `protocole « ${protocol} » invalide : les valeurs acceptées sont TCP, UDP et SCTP.`;
    }

    const name = portName(port);
    if (name.length > 15 || !PORT_NAME.test(name) || !/[a-z]/.test(name)) {
      return `nom de port « ${name} » invalide : 15 caractères au plus, en minuscules, avec au moins une lettre. Exemple : « http ».`;
    }
    if (seenNames.has(name)) {
      return `le nom de port « ${name} » est utilisé deux fois : chaque port doit avoir un nom distinct.`;
    }
    seenNames.add(name);
  }

  // --- variables d'environnement
  for (const [key] of req.env ?? []) {
    if (!ENV_NAME.test(key)) {
      return `variable d'environnement « ${key} » invalide : le nom doit commencer par une lettre ou « _ » et ne contenir que des lettres, chiffres et « _ ».`;
    }
  }

  // --- ressources
  const quantities: [string, string | null | undefined][] = [
    ["cpuRequest", req.cpuRequest],
    ["cpuLimit", req.cpuLimit],
    ["memoryRequest", req.memoryRequest],
    ["memoryLimit", req.memoryLimit],
  ];
  for (const [label, value] of quantities) {
    const v = value?.trim();
    if (v && !isQuantity(v)) {
      return `quantité « ${v} » invalide pour ${label} : utilisez le format Kubernetes, par exemple « 100m », « 0.5 », « 256Mi » ou « 2Gi ».`;
    }
  }

  // --- service
  const serviceType = req.serviceType?.trim() || "ClusterIP";
  if (!SERVICE_TYPES.includes(serviceType)) {
    return `type de Service « ${serviceType} » invalide : les valeurs acceptées sont ${SERVICE_TYPES.join(", ")}.`;
  }

  // --- ingress
  const host = req.ingressHost?.trim();
  if (host) {
    if (host.length > 253 || !HOSTNAME.test(host)) {
      return `nom d'hôte « ${host} » invalide : indiquez un nom DNS, par exemple « api.exemple.fr ».`;
    }
    if ((req.ports ?? []).length === 0) {
      return "un Ingress a été demandé mais aucun port n'est exposé : ajoutez au moins un port conteneur pour que le trafic puisse être routé.";
    }
  }
  const labelErrors = [
    optionalLabel("classe d'Ingress", req.ingressClass),
    optionalLabel("secret TLS d'Ingress", req.ingressTlsSecret),
    optionalLabel("secret de registre", req.imagePullSecret),
    optionalLabel("compte de service", req.serviceAccount),
  ];
  for (const e of labelErrors) if (e) return e;

  // --- volume persistant
  const pvc = req.pvc;
  if (pvc) {
    const size = pvc.size.trim();
    if (!size) return "taille de volume manquante : indiquez par exemple « 10Gi ».";
    if (!isQuantity(size)) {
      return `taille de volume « ${size} » invalide : utilisez une quantité Kubernetes, par exemple « 10Gi » ou « 500Mi ».`;
    }
    const mountPath = pvc.mountPath.trim();
    if (!mountPath.startsWith("/")) {
      return `point de montage « ${mountPath} » invalide : il doit être un chemin absolu, par exemple « /var/lib/données ».`;
    }
    if (mountPath === "/") {
      return "point de montage « / » refusé : monter un volume sur la racine rend le conteneur inutilisable.";
    }
    const name = pvcName(pvc, req.name);
    if (!isDns1123Label(name)) return `nom de volume « ${name} » invalide : il doit respecter le format DNS-1123.`;
    const classError = optionalLabel("classe de stockage", pvc.storageClass);
    if (classError) return classError;
    const mode = pvc.accessMode?.trim() || "ReadWriteOnce";
    if (!ACCESS_MODES.includes(mode)) {
      return `mode d'accès « ${mode} » invalide : les valeurs acceptées sont ${ACCESS_MODES.join(", ")}.`;
    }
  }

  // --- étiquettes, annotations et placement
  const keys = [...Object.keys(req.labels ?? {}), ...Object.keys(req.annotations ?? {}), ...Object.keys(req.nodeSelector ?? {})];
  for (const key of keys) {
    const e = metadataKey(key);
    if (e) return e;
  }

  return null;
}
