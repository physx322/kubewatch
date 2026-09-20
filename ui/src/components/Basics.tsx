import type { Icon } from "@phosphor-icons/react";
import type { ReactNode } from "react";
import { readyTone, statusTone, type Tone } from "@/lib/format";

export function Badge({ tone = "neutral", children, title }: { tone?: Tone; children: ReactNode; title?: string }) {
  const cls = tone === "neutral" ? "badge" : `badge badge-${tone}`;
  return (
    <span className={cls} title={title}>
      {children}
    </span>
  );
}

export function StatusBadge({ status }: { status: string | null | undefined }) {
  if (!status) return <span className="faint">–</span>;
  return <Badge tone={statusTone(status)}>{status}</Badge>;
}

export function ReadyBadge({ ready }: { ready: string | null | undefined }) {
  if (!ready) return <span className="faint">–</span>;
  return <Badge tone={readyTone(ready)}>{ready}</Badge>;
}

export function Dot({ tone = "neutral" }: { tone?: Tone }) {
  return <span className={`dot ${tone === "neutral" || tone === "info" ? "" : "dot-" + tone}`} />;
}

export function EmptyState({
  icon: IconCmp,
  title,
  hint,
  action,
}: {
  icon?: Icon;
  title: string;
  hint?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="empty">
      {IconCmp && <IconCmp size={40} weight="thin" />}
      <h3>{title}</h3>
      {hint && <div className="small">{hint}</div>}
      {action && <div className="mt-8">{action}</div>}
    </div>
  );
}

export function Spinner({ label }: { label?: string }) {
  return (
    <span className="row gap-4 muted small">
      <span className="spinner" />
      {label}
    </span>
  );
}

export function Alert({ tone = "info", children }: { tone?: "info" | "warn" | "err" | "ok"; children: ReactNode }) {
  return <div className={`alert alert-${tone}`}>{children}</div>;
}

export function Meter({ value, warnAt = 75, errAt = 90 }: { value: number | null; warnAt?: number; errAt?: number }) {
  const v = value ?? 0;
  const cls = v >= errAt ? "meter err" : v >= warnAt ? "meter warn" : "meter";
  return (
    <div className={cls} title={value == null ? "indisponible" : `${v.toFixed(0)} %`}>
      <span style={{ width: `${Math.min(100, v)}%` }} />
    </div>
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd>{children}</kbd>;
}

export function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="field">
      <label>{label}</label>
      {children}
      {hint && <div className="hint">{hint}</div>}
    </div>
  );
}

export function Section({ title, children, actions }: { title: string; children: ReactNode; actions?: ReactNode }) {
  return (
    <div className="card">
      <div className="card-header">
        <span className="grow">{title}</span>
        {actions}
      </div>
      <div className="card-body col gap-12">{children}</div>
    </div>
  );
}
