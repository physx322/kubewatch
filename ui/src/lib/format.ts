// Mise en forme : durées, tailles, dates, statuts.

export function age(seconds: number | null | undefined): string {
  if (seconds == null || Number.isNaN(seconds)) return "–";
  const s = Math.max(0, Math.floor(seconds));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d > 0) return `${d}j${h > 0 ? h + "h" : ""}`;
  if (h > 0) return `${h}h${m > 0 ? m + "m" : ""}`;
  if (m > 0) return `${m}m`;
  return `${s}s`;
}

export function ageFromDate(iso: string | null | undefined): string {
  if (!iso) return "–";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "–";
  return age((Date.now() - t) / 1000);
}

export function relativeTime(iso: string | null | undefined): string {
  if (!iso) return "–";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "–";
  const diff = (Date.now() - t) / 1000;
  if (diff < 45) return "à l'instant";
  return `il y a ${age(diff)}`;
}

export function dateTime(iso: string | null | undefined): string {
  if (!iso) return "–";
  const t = new Date(iso);
  if (Number.isNaN(t.getTime())) return iso;
  return t.toLocaleString("fr-FR", { dateStyle: "short", timeStyle: "medium" });
}

export function bytes(n: number | null | undefined): string {
  if (n == null || Number.isNaN(n)) return "–";
  const units = ["o", "Kio", "Mio", "Gio", "Tio"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 && i > 0 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

export function millicores(m: number | null | undefined): string {
  if (m == null || Number.isNaN(m)) return "–";
  if (m >= 1000) return `${(m / 1000).toFixed(m >= 10000 ? 0 : 1)} cœurs`;
  return `${Math.round(m)} m`;
}

export function percent(used: number, capacity: number): number | null {
  if (!capacity || capacity <= 0) return null;
  return Math.max(0, Math.min(100, (used / capacity) * 100));
}

export function count(n: number, singular: string, plural = singular + "s"): string {
  return `${n} ${n > 1 ? plural : singular}`;
}

export type Tone = "ok" | "warn" | "err" | "neutral" | "info";

const OK = /^(running|succeeded|completed?|active|ready|bound|available|normal|healthy|true)$/i;
const WARN = /^(pending|containercreating|podinitializing|terminating|unknown|scheduled|progressing|warning|notready|released|waiting|init:\d+\/\d+)$/i;
const ERR =
  /(failed|error|crashloopbackoff|imagepullbackoff|errimagepull|evicted|oomkilled|backoff|unschedulable|lost|invalid|createcontainerconfigerror|deadlineexceeded)/i;

export function statusTone(status: string | null | undefined): Tone {
  if (!status) return "neutral";
  if (ERR.test(status)) return "err";
  if (OK.test(status)) return "ok";
  if (WARN.test(status)) return "warn";
  return "neutral";
}

export function readyTone(ready: string | null | undefined): Tone {
  if (!ready) return "neutral";
  const m = /^(\d+)\/(\d+)$/.exec(ready);
  if (!m) return "neutral";
  const a = Number(m[1]);
  const b = Number(m[2]);
  if (b === 0) return "neutral";
  if (a === b) return "ok";
  if (a === 0) return "err";
  return "warn";
}

export function shortImage(image: string): string {
  const noDigest = image.split("@")[0] ?? image;
  const parts = noDigest.split("/");
  return parts.length > 2 ? parts.slice(-2).join("/") : noDigest;
}

export function pluralLabel(kind: string): string {
  return kind;
}

export function clip(text: string, max: number): string {
  if (text.length <= max) return text;
  return text.slice(0, max) + "…";
}
