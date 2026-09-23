// ---------------------------------------------------------------------------
// Types miroirs des structures Rust (sérialisées en camelCase par serde).
// Cœur Kubernetes, flux, assistant IA. Les types du hub et de l'updater sont
// dans hub.ts et updates.ts.
//
// Sur le fil, le cœur omet les champs `Option` à `None` et certains tableaux
// vides ; client.ts comble ces absences avant de livrer les objets, si bien
// que les champs déclarés ici sont réellement présents (`null`, `[]`, `{}`).
// ---------------------------------------------------------------------------

export interface ResourceRef {
  group: string;
  version: string;
  kind: string;
  plural: string;
  namespace: string | null;
  name: string;
}

export interface ResourceKind {
  group: string;
  version: string;
  kind: string;
  plural: string;
  namespaced: boolean;
  verbs: string[];
  shortNames: string[];
  categories: string[];
}

export interface OwnerRef {
  kind: string;
  name: string;
  uid: string | null;
  controller: boolean;
}

export interface ObjectSummary {
  name: string;
  namespace: string | null;
  kind: string;
  apiVersion: string;
  uid: string | null;
  createdAt: string | null;
  ageSeconds: number | null;
  status: string;
  ready: string | null;
  restarts: number | null;
  node: string | null;
  images: string[];
  labels: Record<string, string>;
  annotations: Record<string, string>;
  owners: OwnerRef[];
  extra: Record<string, unknown>;
}

export interface ObjectListPage {
  items: ObjectSummary[];
  continueToken: string | null;
  remaining: number | null;
}

export interface ListOptions {
  namespace?: string | null;
  labelSelector?: string | null;
  fieldSelector?: string | null;
  limit?: number | null;
  continueToken?: string | null;
}

export interface ContextInfo {
  name: string;
  cluster: string;
  user: string | null;
  namespace: string | null;
  server: string | null;
  current: boolean;
}

export interface ClusterInfo {
  name: string;
  context: string | null;
  server: string;
  version: string | null;
  platform: string | null;
  connected: boolean;
  nodeCount: number | null;
  namespaceCount: number | null;
  defaultNamespace: string;
  metricsAvailable: boolean;
  lastError: string | null;
}

export interface MetricsSample {
  name: string;
  namespace: string | null;
  container: string | null;
  cpuMillis: number;
  memoryBytes: number;
  timestamp: string | null;
}

export interface ClusterOverview {
  nodesTotal: number;
  nodesReady: number;
  podsTotal: number;
  podsRunning: number;
  podsPending: number;
  podsFailed: number;
  namespaces: number;
  cpuCapacityMillis: number;
  cpuUsedMillis: number;
  memoryCapacityBytes: number;
  memoryUsedBytes: number;
  workloads: Record<string, number>;
  warnings: string[];
  metricsAvailable: boolean;
}

export interface EventSummary {
  namespace: string | null;
  name: string;
  reason: string;
  message: string;
  type: string;
  count: number;
  firstSeen: string | null;
  lastSeen: string | null;
  involvedKind: string | null;
  involvedName: string | null;
  source: string | null;
}

export interface ContainerInfo {
  name: string;
  image: string;
  ready: boolean;
  restartCount: number;
  state: string;
  init: boolean;
}

export type ConnectionSpec =
  | { type: "kubeconfig"; path?: string | null; context?: string | null }
  | { type: "inline"; yaml: string; context?: string | null }
  | {
      type: "remote";
      server: string;
      token?: string | null;
      caCertPem?: string | null;
      clientCertPem?: string | null;
      clientKeyPem?: string | null;
      insecureSkipTlsVerify?: boolean;
      namespace?: string | null;
      proxyUrl?: string | null;
    }
  | { type: "inCluster" };

export interface LogOptions {
  container?: string | null;
  follow?: boolean;
  tailLines?: number | null;
  sinceSeconds?: number | null;
  timestamps?: boolean;
  previous?: boolean;
}

export type ApplyAction = "created" | "configured" | "unchanged" | "dryRun" | "deleted" | "failed";

export interface ApplyItem {
  resource: ResourceRef;
  action: ApplyAction;
  message: string | null;
}

export interface ApplyOutcome {
  items: ApplyItem[];
  failed: number;
}

export interface DiffItem {
  resource: ResourceRef;
  diff: string;
}

export interface GraphSnapshot {
  objects: ObjectSummary[];
  warnings: string[];
}

export interface MetricsBundle {
  nodes: MetricsSample[];
  pods: MetricsSample[];
}

export type LogEvent = { type: "lines"; lines: string[] } | { type: "ended"; error: string | null };

export type ExecEvent = { type: "output"; data: string } | { type: "ended"; message: string | null };

export interface AppInfo {
  version: string;
  stateDir: string;
  warnings: string[];
}

/** Accélération matérielle demandée pour la fenêtre. */
export type Acceleration = "auto" | "full" | "cpuPainting" | "off";

/** Rendu réellement en vigueur depuis le lancement. */
export type AppliedRender = "gpu" | "hybrid" | "cpuPainting" | "noDmabuf" | "software";

/** Rastérisation du texte ; s'applique à chaud. */
export type TextRendering = "system" | "sharp" | "smooth";

export interface RenderView {
  acceleration: Acceleration;
  text: TextRendering;
  applied: AppliedRender;
  /** Une variable d'environnement impose le mode : le réglage est sans effet. */
  forcedByEnv: boolean;
  /** Le choix enregistré attend un redémarrage. */
  restartNeeded: boolean;
  /** Faux là où la plateforme ne sait rien faire de ce réglage. */
  supported: boolean;
}

// --- Assistant IA -----------------------------------------------------------

export type ProviderKind = "anthropic" | "openAi" | "openAiCompatible";

export interface ProfileView {
  id: string;
  name: string;
  kind: ProviderKind;
  baseUrl: string;
  model: string;
  apiKeySet: boolean;
  maxOutputTokens: number;
  showThinking: boolean;
  /** Le profil emprunte les identifiants de Claude Code (`~/.claude`). */
  useClaudeCode: boolean;
}

/** Compte laissé par Claude Code sur la machine. Jamais de secret ici. */
export interface ClaudeCodeAccount {
  dir: string;
  source: string;
  kind: "oauth" | "apiKey";
  email: string | null;
  organization: string | null;
  subscription: string | null;
  /** Millisecondes depuis l'époque Unix. */
  expiresAt: number | null;
  expired: boolean;
  baseUrl: string | null;
  model: string | null;
}

export interface AiSettingsView {
  profiles: ProfileView[];
  activeProfile: string | null;
  toolsEnabled: boolean;
  maxToolRounds: number;
  extraInstructions: string;
}

/** `apiKey` : absent = inchangée, "" = effacée, texte = remplacée. */
export interface ProfileUpdate {
  id?: string | null;
  name: string;
  kind: ProviderKind;
  baseUrl?: string | null;
  apiKey?: string | null;
  model: string;
  maxOutputTokens?: number | null;
  showThinking?: boolean;
  useClaudeCode?: boolean;
}

export interface ModelInfo {
  id: string;
  displayName: string | null;
}

export type Role = "user" | "assistant";

export type Part =
  | { type: "text"; text: string }
  | { type: "toolCall"; id: string; name: string; input: unknown }
  | { type: "toolResult"; callId: string; name: string; content: string; isError: boolean };

export interface ChatMessage {
  role: Role;
  parts: Part[];
}

export type StopReason = "endTurn" | "toolUse" | "maxTokens" | "refusal" | "other";

export interface Usage {
  inputTokens: number;
  outputTokens: number;
}

export type StreamEvent =
  | { type: "textDelta"; text: string }
  | { type: "thinkingDelta"; text: string }
  | { type: "toolCallStart"; id: string; name: string }
  | { type: "toolCallDelta"; id: string; partialJson: string }
  | { type: "toolCall"; id: string; name: string; input: unknown }
  | { type: "toolResult"; callId: string; name: string; content: string; isError: boolean }
  | {
      type: "turnEnd";
      stopReason: StopReason;
      usage: Usage;
      model: string | null;
      refusalCategory: string | null;
    }
  | { type: "error"; message: string };

export interface AiContext {
  cluster?: string | null;
  namespace?: string | null;
  view?: string | null;
  selection?: string | null;
}

export interface ChatOutcome {
  messages: ChatMessage[];
  stopReason: StopReason;
  usage: Usage;
  model: string | null;
  refusalCategory: string | null;
}
