// Menu contextuel positionné à la souris.
import { useEffect, useRef, type ReactNode } from "react";

export interface MenuItem {
  label: string;
  icon?: ReactNode;
  danger?: boolean;
  disabled?: boolean;
  onSelect?: () => void;
  separator?: boolean;
}

export function ContextMenu({
  at,
  items,
  onClose,
}: {
  at: { x: number; y: number } | null;
  items: MenuItem[];
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!at) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [at, onClose]);
  if (!at) return null;
  const x = Math.min(at.x, window.innerWidth - 220);
  const y = Math.min(at.y, window.innerHeight - items.length * 30 - 16);
  return (
    <div ref={ref} className="menu" style={{ left: x, top: y, position: "fixed" }}>
      {items.map((it, i) =>
        it.separator ? (
          <div key={i} className="menu-sep" />
        ) : (
          <button
            key={i}
            className={`menu-item ${it.danger ? "danger" : ""}`}
            disabled={it.disabled}
            onClick={() => {
              onClose();
              it.onSelect?.();
            }}
          >
            {it.icon}
            {it.label}
          </button>
        ),
      )}
    </div>
  );
}
