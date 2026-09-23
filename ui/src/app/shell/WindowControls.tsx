// ---------------------------------------------------------------------------
// Barre de titre maison. La fenêtre est créée sans décoration système (voir
// `decorations` dans tauri.conf.json) : le déplacement, les boutons et les
// poignées de redimensionnement viennent donc de l'interface.
// ---------------------------------------------------------------------------
import { CopySimple, Minus, Square, X } from "@phosphor-icons/react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";

type AppWindow = ReturnType<typeof getCurrentWindow>;
type ResizeDirection = Parameters<AppWindow["startResizeDragging"]>[0];

/** Le pont Tauri est absent d'un navigateur nu (`npm run dev` sans l'appli). */
function appWindow(): AppWindow | null {
  try {
    return getCurrentWindow();
  } catch {
    return null;
  }
}

export function WindowControls() {
  const [maximized, setMaximized] = useState(false);

  // L'icône du bouton central suit l'état réel : la fenêtre peut être agrandie
  // par le gestionnaire de fenêtres sans passer par le bouton.
  useEffect(() => {
    const win = appWindow();
    if (!win) return;
    let live = true;
    const sync = () => {
      win
        .isMaximized()
        .then((m) => live && setMaximized(m))
        .catch(() => {});
    };
    sync();
    const off = win.onResized(sync);
    return () => {
      live = false;
      off.then((f) => f()).catch(() => {});
    };
  }, []);

  const act = (fn: (w: AppWindow) => Promise<void>) => () => {
    const win = appWindow();
    if (win) fn(win).catch(() => {});
  };

  return (
    <div className="win-controls">
      <button className="win-btn" onClick={act((w) => w.minimize())} title="Réduire" aria-label="Réduire">
        <Minus size={14} weight="bold" />
      </button>
      <button
        className="win-btn"
        onClick={act((w) => w.toggleMaximize())}
        title={maximized ? "Restaurer" : "Agrandir"}
        aria-label={maximized ? "Restaurer" : "Agrandir"}
      >
        {maximized ? <CopySimple size={13} weight="bold" /> : <Square size={12} weight="bold" />}
      </button>
      <button className="win-btn win-close" onClick={act((w) => w.close())} title="Fermer" aria-label="Fermer">
        <X size={14} weight="bold" />
      </button>
    </div>
  );
}

// Une fenêtre sans décoration perd les bords de redimensionnement fournis par
// le système : on les redessine, quelques pixels sur chaque arête et coin.
const GRIPS: { dir: ResizeDirection; side: string }[] = [
  { dir: "North", side: "n" },
  { dir: "South", side: "s" },
  { dir: "West", side: "w" },
  { dir: "East", side: "e" },
  { dir: "NorthWest", side: "nw" },
  { dir: "NorthEast", side: "ne" },
  { dir: "SouthWest", side: "sw" },
  { dir: "SouthEast", side: "se" },
];

export function ResizeGrips() {
  return (
    <>
      {GRIPS.map(({ dir, side }) => (
        <div
          key={side}
          className={`resize-grip rg-${side}`}
          onMouseDown={(e) => {
            if (e.button !== 0) return;
            appWindow()?.startResizeDragging(dir).catch(() => {});
          }}
        />
      ))}
    </>
  );
}
