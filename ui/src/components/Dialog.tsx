import { X } from "@phosphor-icons/react";
import { useEffect, type ReactNode } from "react";

export function Dialog({
  open,
  title,
  onClose,
  children,
  footer,
  size = "md",
  icon,
}: {
  open: boolean;
  title: ReactNode;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  size?: "md" | "lg";
  icon?: ReactNode;
}) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, onClose]);

  if (!open) return null;
  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className={`dialog ${size === "lg" ? "dialog-lg" : ""}`} role="dialog" aria-modal="true">
        <div className="dialog-header">
          {icon}
          <h3 className="grow truncate">{title}</h3>
          <button className="btn btn-ghost btn-icon btn-sm" onClick={onClose} title="Fermer (Échap)">
            <X size={16} />
          </button>
        </div>
        <div className="dialog-body">{children}</div>
        {footer && <div className="dialog-footer">{footer}</div>}
      </div>
    </div>
  );
}
