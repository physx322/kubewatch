// Types du hub (registres d'images, charts, catalogue, déploiement).
// Miroirs de crates/hub/src/model.rs et deploy.rs.

export type RegistryKind = "dockerHub" | "ghcr" | "quay" | "generic";

export interface ImageSummary {
  name: string;
  namespace: string | null;
  description: string | null;
  stars: number | null;
  pulls: number | null;
  official: boolean;
  registry: RegistryKind;
  updatedAt: string | null;
  sourceRepo: string | null;
}

export interface TagInfo {
  name: string;
  digest: string | null;
  sizeBytes: number | null;
  pushedAt: string | null;
  platforms: string[];
  semver: string | null;
}

export interface ImageDetails {
  reference: string;
  digest: string | null;
  exposedPorts: number[];
  env: string[];
  labels: Record<string, string>;
  entrypoint: string[];
  cmd: string[];
  architecture: string | null;
  os: string | null;
  sourceRepo: string | null;
}

export interface ChartSummary {
  name: string;
  repository: string;
  version: string;
  appVersion: string | null;
  description: string | null;
  icon: string | null;
  stars: number | null;
  home: string | null;
  repoUrl: string | null;
}

export interface CatalogApp {
  id: string;
  name: string;
  description: string;
  icon: string | null;
  category: string;
  image: string;
  defaultPort: number | null;
  env: [string, string][];
  needsPvc: boolean;
  chart: ChartSummary | null;
  docsUrl: string | null;
}

export interface PortSpec {
  name?: string | null;
  containerPort: number;
  servicePort?: number | null;
  protocol?: string | null;
}

export interface PvcSpec {
  name?: string | null;
  size: string;
  mountPath: string;
  storageClass?: string | null;
  accessMode?: string | null;
}

/** Tous les champs ont une valeur par défaut côté Rust : n'envoyer que l'utile. */
export interface DeployRequest {
  name: string;
  namespace: string;
  image: string;
  replicas?: number;
  ports?: PortSpec[];
  env?: [string, string][];
  cpuRequest?: string | null;
  cpuLimit?: string | null;
  memoryRequest?: string | null;
  memoryLimit?: string | null;
  serviceType?: string | null;
  ingressHost?: string | null;
  ingressClass?: string | null;
  ingressTlsSecret?: string | null;
  imagePullSecret?: string | null;
  pvc?: PvcSpec | null;
  labels?: Record<string, string>;
  annotations?: Record<string, string>;
  command?: string[];
  args?: string[];
  serviceAccount?: string | null;
  nodeSelector?: Record<string, string>;
  [extra: string]: unknown;
}
