// Types du moteur de mise à jour. Miroirs de crates/updater/src/{model,scan,store}.rs.

export type UpdateSource =
  | { type: "githubRelease"; owner: string; repo: string; tagPrefix?: string | null }
  | { type: "containerRegistry"; image: string }
  | { type: "helmChart"; repo: string; chart: string };

export type UpdateChannel = "major" | "minor" | "patch" | "prerelease" | "pinned";

export interface UpdatePolicy {
  channel: UpdateChannel;
  allowPrerelease: boolean;
  constraint?: string | null;
  ignore: string[];
  autoApply: boolean;
  checkIntervalSeconds: number;
  maintenanceWindow?: string | null;
}

/** Miroir de `UpdatePolicy::default()` côté Rust (canal « correctif », 1 h). */
export const DEFAULT_POLICY: UpdatePolicy = {
  channel: "patch",
  allowPrerelease: false,
  constraint: null,
  ignore: [],
  autoApply: false,
  checkIntervalSeconds: 3600,
  maintenanceWindow: null,
};

export interface WatchTarget {
  namespace?: string | null;
  kind: string;
  name: string;
}

export interface WatcherSpec {
  id: string;
  name: string;
  enabled: boolean;
  cluster: string;
  target: WatchTarget;
  container?: string | null;
  source: UpdateSource;
  policy: UpdatePolicy;
  createdAt: string;
  lastCheckedAt?: string | null;
  lastKnownVersion?: string | null;
}

export interface ReleaseAsset {
  name: string;
  size: number;
  downloadUrl: string;
  contentType?: string | null;
}

export interface ReleaseInfo {
  tag: string;
  name?: string | null;
  body?: string | null;
  publishedAt?: string | null;
  prerelease: boolean;
  draft: boolean;
  htmlUrl?: string | null;
  semver?: string | null;
  assets: ReleaseAsset[];
}

export type UpdateSeverity = "major" | "minor" | "patch" | "unknown";

export interface UpdateFinding {
  id: string;
  watcherId: string;
  watcherName: string;
  cluster: string;
  targetKind: string;
  targetNamespace?: string | null;
  targetName: string;
  container?: string | null;
  currentVersion?: string | null;
  currentImage?: string | null;
  availableVersion: string;
  availableImage?: string | null;
  source: UpdateSource;
  severity: UpdateSeverity;
  release?: ReleaseInfo | null;
  detectedAt: string;
  applied: boolean;
}

export interface RolloutStatus {
  readyReplicas: number;
  updatedReplicas: number;
  desiredReplicas: number;
  available: boolean;
  message: string;
}

/**
 * Résultat d'une application (ou d'un retour arrière) de mise à jour.
 * `status` vaut `"applied"`, `"rolledBack"` ou `"failed: …"` ; `findingId` est
 * vide pour un retour arrière manuel.
 */
export interface RolloutResult {
  findingId: string;
  target: WatchTarget;
  previousImage?: string | null;
  newImage: string;
  appliedAt: string;
  status: string;
}

export const ROLLOUT_STATUS_APPLIED = "applied";
export const ROLLOUT_STATUS_ROLLED_BACK = "rolledBack";
export const ROLLOUT_STATUS_FAILED = "failed";

/** Miroir de `RolloutResult::succeeded()`. */
export function rolloutSucceeded(r: RolloutResult): boolean {
  return !r.status.startsWith(ROLLOUT_STATUS_FAILED);
}

export interface WorkloadImage {
  cluster: string;
  namespace: string | null;
  kind: string;
  name: string;
  container: string;
  image: string;
  sourceRepoGuess: string | null;
}

/** Les secrets arrivent masqués ; renvoyer le masque tel quel signifie « inchangé ». */
export interface UpdaterSettings {
  githubToken: string | null;
  webhookSecret: string | null;
  defaultPolicy: UpdatePolicy;
  schedulerEnabled: boolean;
}

export interface WatchersBundle {
  watchers: WatcherSpec[];
  findings: UpdateFinding[];
}
