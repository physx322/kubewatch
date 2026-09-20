import { CheckCircle, Info, Warning, WarningCircle, X } from "@phosphor-icons/react";
import { useStore } from "../store";

const ICONS = {
  ok: <CheckCircle size={18} color="var(--ok)" weight="fill" />,
  err: <WarningCircle size={18} color="var(--err)" weight="fill" />,
  warn: <Warning size={18} color="var(--warn)" weight="fill" />,
  info: <Info size={18} color="var(--info)" weight="fill" />,
};

export function Toasts() {
  const toasts = useStore((s) => s.toasts);
  const dismiss = useStore((s) => s.dismissToast);
  if (toasts.length === 0) return null;
  return (
    <div className="toasts">
      {toasts.map((t) => (
        <div key={t.id} className={`toast toast-${t.kind}`}>
          {ICONS[t.kind]}
          <div className="grow">
            {t.title && <div style={{ fontWeight: 600 }}>{t.title}</div>}
            <div style={{ whiteSpace: "pre-wrap" }}>{t.message}</div>
          </div>
          <button className="btn btn-ghost btn-icon btn-sm" onClick={() => dismiss(t.id)}>
            <X size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}
