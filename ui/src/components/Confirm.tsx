// Confirmation modale pilotée par une promesse : `await confirm({...})`.
import { Warning } from "@phosphor-icons/react";
import { create } from "zustand";
import { Dialog } from "./Dialog";

interface Request {
  title: string;
  message: string;
  confirmLabel?: string;
  danger?: boolean;
  resolve: (ok: boolean) => void;
}

const useConfirmStore = create<{ current: Request | null; ask: (r: Request) => void; done: (ok: boolean) => void }>(
  (set, get) => ({
    current: null,
    ask: (r) => set({ current: r }),
    done: (ok) => {
      get().current?.resolve(ok);
      set({ current: null });
    },
  }),
);

export function confirm(opts: Omit<Request, "resolve">): Promise<boolean> {
  return new Promise((resolve) => useConfirmStore.getState().ask({ ...opts, resolve }));
}

export function ConfirmHost() {
  const current = useConfirmStore((s) => s.current);
  const done = useConfirmStore((s) => s.done);
  return (
    <Dialog
      open={!!current}
      title={current?.title ?? ""}
      onClose={() => done(false)}
      icon={current?.danger ? <Warning size={20} color="var(--err)" /> : undefined}
      footer={
        <>
          <button className="btn" onClick={() => done(false)}>
            Annuler
          </button>
          <button className={`btn ${current?.danger ? "btn-danger" : "btn-primary"}`} onClick={() => done(true)} autoFocus>
            {current?.confirmLabel ?? "Confirmer"}
          </button>
        </>
      }
    >
      <p style={{ margin: 0, whiteSpace: "pre-wrap" }} className="selectable">
        {current?.message}
      </p>
    </Dialog>
  );
}
