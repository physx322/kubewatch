// Éléments communs de l'écran « Mises à jour » : libellés, badges et
// petites transformations partagées entre la vue et le dialogue de surveillant.
import type { Tone } from "@/lib/format";
import type {
  RolloutResult,
  UpdateChannel,
  UpdateSeverity,
  UpdateSource,
  WatchTarget,
} from "@/api/updates";
import { rolloutSucceeded, ROLLOUT_STATUS_APPLIED, ROLLOUT_STATUS_ROLLED_BACK } from "@/api/updates";
import { Badge } from "./Basics";

/** Miroir de `UpdateSource::label()` : `github:owner/repo`, `image:…`, `helm:repo/chart`. */
export function sourceLabel(s: UpdateSource): string {
  switch (s.type) {
    case "githubRelease":
      return `github:${s.owner}/${s.repo}`;
    case "containerRegistry":
      return `image:${s.image}`;
    case "helmChart":
      return `helm:${s.repo}/${s.chart}`;
  }
}

/** Miroir de `WatchTarget::label()` : `namespace/Kind/nom` ou `Kind/nom`. */
export function targetLabel(t: WatchTarget): string {
  return t.namespace ? `${t.namespace}/${t.kind}/${t.name}` : `${t.kind}/${t.name}`;
}

export const CHANNELS: { id: UpdateChannel; label: string; hint: string }[] = [
  { id: "major", label: "Majeure", hint: "Toute version supérieure, changement de majeure compris." },
  { id: "minor", label: "Mineure", hint: "Même majeure, mineure ou correctif supérieur." },
  { id: "patch", label: "Correctif", hint: "Même majeure et même mineure, correctif supérieur." },
  { id: "prerelease", label: "Pré-version", hint: "Comme « majeure », pré-versions acceptées." },
  { id: "pinned", label: "Figée", hint: "Aucune mise à jour proposée." },
];

export function channelLabel(c: UpdateChannel): string {
  return CHANNELS.find((x) => x.id === c)?.label ?? c;
}

const SEVERITY: Record<UpdateSeverity, { label: string; tone: Tone }> = {
  major: { label: "majeure", tone: "err" },
  minor: { label: "mineure", tone: "warn" },
  patch: { label: "correctif", tone: "ok" },
  unknown: { label: "inconnue", tone: "neutral" },
};

export function SeverityBadge({ severity }: { severity: UpdateSeverity }) {
  const s = SEVERITY[severity] ?? SEVERITY.unknown;
  return (
    <Badge tone={s.tone} title="Ampleur du saut de version">
      {s.label}
    </Badge>
  );
}

/** Étiquette monospace d'une source, avec le libellé complet en infobulle. */
export function SourceTag({ source }: { source: UpdateSource }) {
  const label = sourceLabel(source);
  return (
    <span className="tag" title={label} style={{ maxWidth: 320 }}>
      <span className="truncate" style={{ minWidth: 0 }}>
        {label}
      </span>
    </span>
  );
}

/** Statut d'un déploiement passé : `applied`, `rolledBack` ou `failed: …`. */
export function RolloutStatusBadge({ result }: { result: RolloutResult }) {
  if (result.status === ROLLOUT_STATUS_APPLIED) return <Badge tone="ok">appliquée</Badge>;
  if (result.status === ROLLOUT_STATUS_ROLLED_BACK) return <Badge tone="info">retour arrière</Badge>;
  const ok = rolloutSucceeded(result);
  const detail = result.status.replace(/^failed:?\s*/, "");
  return (
    <Badge tone={ok ? "neutral" : "err"} title={result.status}>
      {ok ? result.status : detail ? `échec : ${detail}` : "échec"}
    </Badge>
  );
}

/** Durée lisible pour un intervalle en secondes (« 1 h », « 2 j 6 h »…). */
export function humanInterval(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  const parts: string[] = [];
  if (d) parts.push(`${d} j`);
  if (h) parts.push(`${h} h`);
  if (m) parts.push(`${m} min`);
  if (parts.length === 0) parts.push(`${s} s`);
  return parts.join(" ");
}

/** Pluriel Kubernetes minuscule attendu par l'écran Ressources (`Deployment` → `deployments`). */
export function resourcesKindOf(kind: string): string {
  return kind.toLowerCase() + "s";
}

/** UUID v4 pour un nouveau surveillant (le backend accepte l'identifiant fourni). */
export function newId(): string {
  const c = globalThis.crypto;
  if (c && typeof c.randomUUID === "function") return c.randomUUID();
  const b = new Uint8Array(16);
  c.getRandomValues(b);
  b[6] = (b[6]! & 0x0f) | 0x40;
  b[8] = (b[8]! & 0x3f) | 0x80;
  const hex = Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
