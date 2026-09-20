// Session interactive dans un conteneur : xterm.js relié au flux `start_exec`.
//
// Portage de `crates/desktop/src/widgets/term.rs` : l'émulation ANSI, la grille
// et la traduction des touches sont confiées à xterm.js ; le composant garde la
// barre d'outils, le cycle de vie de la session et l'adaptation à la taille.
import { ArrowsClockwise, Copy, Plugs, PlugsConnected, TerminalWindow, Trash } from "@phosphor-icons/react";
import { useQuery } from "@tanstack/react-query";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal, type ITheme } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useCallback, useEffect, useRef, useState } from "react";
import { api, base64ToBytes } from "@/api/client";
import type { ResourceRef } from "@/api/types";
import { notify } from "@/app/store";
import { useDarkMode } from "@/app/theme";
import { Badge, Spinner } from "./Basics";
import "./TerminalPane.css";

/** Lignes conservées au-dessus de l'écran. */
const SCROLLBACK = 2000;

/** Bornes de la grille, pour ne jamais demander au cluster une taille absurde. */
const MIN_COLS = 8;
const MAX_COLS = 500;
const MIN_ROWS = 2;
const MAX_ROWS = 300;

type Shell = "auto" | "/bin/bash" | "/bin/sh" | "custom";
type Status = "idle" | "connecting" | "open" | "closed";

const SHELLS: [Shell, string][] = [
  ["auto", "Interpréteur automatique"],
  ["/bin/bash", "/bin/bash"],
  ["/bin/sh", "/bin/sh"],
  ["custom", "Commande personnalisée…"],
];

/** Découpe une ligne de commande en arguments, en respectant les guillemets simples et doubles. */
function splitCommand(input: string): string[] {
  const out: string[] = [];
  const re = /"([^"]*)"|'([^']*)'|(\S+)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(input))) out.push(m[1] ?? m[2] ?? m[3] ?? "");
  return out;
}

function clamp(n: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, n));
}

/** Apparence de xterm dérivée des jetons CSS du thème courant. */
function readLook(): { theme: ITheme; fontFamily: string; fontSize: number } {
  const cs = getComputedStyle(document.documentElement);
  const v = (name: string) => cs.getPropertyValue(name).trim() || undefined;
  const bg = v("--log-bg");
  const accent = v("--accent");
  return {
    theme: {
      background: bg,
      foreground: v("--log-fg"),
      cursor: accent,
      cursorAccent: bg,
      selectionBackground: v("--accent-ring") ?? accent,
      selectionInactiveBackground: v("--accent-ring") ?? accent,
    },
    fontFamily: v("--font-mono") ?? "monospace",
    fontSize: Math.round(parseFloat(v("--fs-s") ?? "")) || 13,
  };
}

/** Écran et historique en texte brut, sans les espaces de fin de ligne. */
function bufferText(term: Terminal): string {
  const buffer = term.buffer.active;
  const lines: string[] = [];
  for (let i = 0; i < buffer.length; i++) lines.push(buffer.getLine(i)?.translateToString(true) ?? "");
  return lines.join("\n").replace(/\n+$/, "");
}

async function copySelection(term: Terminal, toast: boolean) {
  const text = term.hasSelection() ? term.getSelection() : bufferText(term);
  if (!text) return;
  try {
    await navigator.clipboard.writeText(text);
    if (toast) notify.ok(term.hasSelection() ? "Sélection copiée." : "Contenu du terminal copié.");
  } catch {
    notify.err("Copie impossible.");
  }
}

async function pasteInto(term: Terminal) {
  try {
    const text = await navigator.clipboard.readText();
    if (text) term.paste(text);
    return;
  } catch {
    // Lecture asynchrone refusée : on retombe sur l'évènement « paste » natif,
    // que xterm.js sait traiter.
  }
  term.focus();
  if (!document.execCommand("paste")) notify.warn("Lecture du presse-papiers impossible.");
}

export function TerminalPane({ cluster, pod, container }: { cluster: string; pod: ResourceRef; container: string | null }) {
  const dark = useDarkMode();
  const containers = useQuery({
    queryKey: ["containers", cluster, pod.namespace, pod.name],
    queryFn: () => api.resources.containers(cluster, pod),
  });

  const [selected, setSelected] = useState<string>("");
  const [shell, setShell] = useState<Shell>("auto");
  const [custom, setCustom] = useState("");
  const [status, setStatus] = useState<Status>("idle");
  const [closeMessage, setCloseMessage] = useState<string | null>(null);
  const [size, setSize] = useState({ cols: 80, rows: 24 });
  const [focused, setFocused] = useState(false);

  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const sessionRef = useRef<number | null>(null);
  /** Taille déjà annoncée au pseudo-terminal, pour ne pas la répéter. */
  const sentSize = useRef<{ cols: number; rows: number } | null>(null);
  /** Incrémenté à chaque ouverture ou fermeture : les évènements d'une session périmée sont ignorés. */
  const generation = useRef(0);
  const fitFrame = useRef<number | null>(null);

  // Conteneur par défaut : la pré-sélection si elle existe, sinon le premier conteneur non-init.
  useEffect(() => {
    if (!containers.data || selected) return;
    const wanted = container ? containers.data.find((c) => c.name === container) : undefined;
    const main = wanted ?? containers.data.find((c) => !c.init) ?? containers.data[0];
    if (main) setSelected(main.name);
  }, [containers.data, container, selected]);

  // --- Taille de la grille --------------------------------------------------

  const fitNow = useCallback(() => {
    const term = termRef.current;
    const fit = fitRef.current;
    if (!term || !fit) return;
    const dims = fit.proposeDimensions();
    if (!dims || !Number.isFinite(dims.cols) || !Number.isFinite(dims.rows)) return;
    const cols = clamp(dims.cols, MIN_COLS, MAX_COLS);
    const rows = clamp(dims.rows, MIN_ROWS, MAX_ROWS);
    if (cols !== term.cols || rows !== term.rows) term.resize(cols, rows);
    const id = sessionRef.current;
    if (id != null && (sentSize.current?.cols !== cols || sentSize.current?.rows !== rows)) {
      sentSize.current = { cols, rows };
      api.exec.resize(id, cols, rows).catch(() => {});
    }
  }, []);

  const scheduleFit = useCallback(() => {
    if (fitFrame.current != null) return;
    fitFrame.current = requestAnimationFrame(() => {
      fitFrame.current = null;
      fitNow();
    });
  }, [fitNow]);

  // --- Session ----------------------------------------------------------------

  const disconnect = useCallback(async (message: string | null = null) => {
    generation.current += 1;
    const id = sessionRef.current;
    sessionRef.current = null;
    sentSize.current = null;
    setStatus((prev) => (prev === "idle" ? "idle" : "closed"));
    setCloseMessage(message);
    if (id != null) {
      termRef.current?.write("\r\n\x1b[2m— session fermée —\x1b[0m\r\n");
      await api.exec.stop(id).catch(() => {});
    }
  }, []);

  const connect = useCallback(async () => {
    const term = termRef.current;
    if (!term || !selected) return;
    await disconnect();
    const gen = ++generation.current;
    const command = shell === "auto" ? [] : shell === "custom" ? splitCommand(custom) : [shell];

    term.reset();
    fitNow();
    const { cols, rows } = term;
    setStatus("connecting");
    setCloseMessage(null);
    try {
      const id = await api.exec.start(cluster, pod, selected, command, { cols, rows }, (ev) => {
        if (generation.current !== gen) return;
        if (ev.type === "output") {
          termRef.current?.write(base64ToBytes(ev.data));
        } else {
          sessionRef.current = null;
          sentSize.current = null;
          setStatus("closed");
          setCloseMessage(ev.message);
          termRef.current?.write("\r\n\x1b[2m— session fermée —\x1b[0m\r\n");
          if (ev.message) notify.err(`Terminal fermé : ${ev.message}`);
        }
      });
      if (generation.current !== gen) {
        // Fermée (ou pod changé) pendant l'ouverture : on ne laisse pas d'exec orphelin.
        api.exec.stop(id).catch(() => {});
        return;
      }
      sessionRef.current = id;
      sentSize.current = { cols, rows };
      setStatus("open");
      term.focus();
      // La grille a pu changer pendant l'ouverture : on l'annonce si besoin.
      fitNow();
    } catch (e) {
      if (generation.current !== gen) return;
      const message = (e as Error).message;
      setStatus("closed");
      setCloseMessage(message);
      notify.err(message);
    }
  }, [cluster, pod, selected, shell, custom, disconnect, fitNow]);

  // Changer de conteneur ferme la session en cours : jamais d'exec orphelin.
  const changeContainer = (name: string) => {
    setSelected(name);
    if (sessionRef.current != null || status === "connecting") disconnect();
  };

  // --- Cycle de vie de xterm --------------------------------------------------

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const look = readLook();
    const term = new Terminal({
      cursorBlink: true,
      scrollback: SCROLLBACK,
      fontFamily: look.fontFamily,
      fontSize: look.fontSize,
      theme: look.theme,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    termRef.current = term;
    fitRef.current = fit;

    const subs = [
      term.onData((data) => {
        const id = sessionRef.current;
        if (id != null) api.exec.input(id, data).catch(() => {});
      }),
      term.onBinary((data) => {
        const id = sessionRef.current;
        if (id != null) api.exec.input(id, Uint8Array.from(data, (c) => c.charCodeAt(0) & 0xff)).catch(() => {});
      }),
      term.onResize(({ cols, rows }) => setSize({ cols, rows })),
    ];
    // Ctrl+Maj+C copie la sélection, Ctrl+Maj+V colle ; tout le reste part au shell distant.
    term.attachCustomKeyEventHandler((ev) => {
      if (!ev.ctrlKey || !ev.shiftKey || ev.altKey || ev.metaKey) return true;
      const key = ev.key.toLowerCase();
      if (key !== "c" && key !== "v") return true;
      ev.preventDefault();
      if (ev.type === "keydown") {
        if (key === "c") copySelection(term, false);
        else pasteInto(term);
      }
      return false;
    });
    const textarea = term.textarea;
    const onFocus = () => setFocused(true);
    const onBlur = () => setFocused(false);
    textarea?.addEventListener("focus", onFocus);
    textarea?.addEventListener("blur", onBlur);

    const observer = new ResizeObserver(() => scheduleFit());
    observer.observe(host);
    fitNow();
    // Une seconde mesure quand les polices sont prêtes : la largeur des glyphes peut changer.
    document.fonts?.ready.then(() => scheduleFit()).catch(() => {});

    return () => {
      observer.disconnect();
      if (fitFrame.current != null) {
        cancelAnimationFrame(fitFrame.current);
        fitFrame.current = null;
      }
      textarea?.removeEventListener("focus", onFocus);
      textarea?.removeEventListener("blur", onBlur);
      for (const s of subs) s.dispose();
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
  }, [fitNow, scheduleFit]);

  // Thème : relu après que l'attribut data-theme a été posé sur <html> (d'où le report d'une image).
  useEffect(() => {
    void dark;
    const frame = requestAnimationFrame(() => {
      const term = termRef.current;
      if (!term) return;
      const look = readLook();
      term.options.theme = look.theme;
      term.options.fontFamily = look.fontFamily;
      term.options.fontSize = look.fontSize;
      scheduleFit();
    });
    return () => cancelAnimationFrame(frame);
  }, [dark, scheduleFit]);

  // Changement de pod : la session est fermée et l'écran remis à zéro.
  const podKey = `${cluster}/${pod.namespace ?? ""}/${pod.name}`;
  const previousKey = useRef(podKey);
  useEffect(() => {
    if (previousKey.current === podKey) return;
    previousKey.current = podKey;
    disconnect();
    setSelected("");
    setStatus("idle");
    setCloseMessage(null);
    termRef.current?.reset();
  }, [podKey, disconnect]);

  // Démontage : jamais d'exec orphelin derrière soi.
  useEffect(() => {
    return () => {
      disconnect();
    };
  }, [disconnect]);

  // --- Rendu ------------------------------------------------------------------

  const info = containers.data?.find((c) => c.name === selected);
  const ordered = containers.data ? [...containers.data.filter((c) => !c.init), ...containers.data.filter((c) => c.init)] : [];
  const busy = status === "connecting";
  const open = status === "open";
  const canConnect = !!selected && !containers.isLoading && !(shell === "custom" && !custom.trim());

  return (
    <div className="term-pane">
      <div className="toolbar" style={{ flexWrap: "wrap" }}>
        <select className="select select-sm" value={selected} onChange={(e) => changeContainer(e.target.value)} title="Conteneur" disabled={containers.isLoading}>
          {containers.isLoading && <option value="">chargement…</option>}
          {ordered.map((c) => (
            <option key={c.name} value={c.name}>
              {c.init ? "init: " : ""}
              {c.name}
            </option>
          ))}
        </select>
        {info && (
          <span className="muted xs nowrap" title="État du conteneur">
            {info.state}
          </span>
        )}
        <select className="select select-sm" value={shell} onChange={(e) => setShell(e.target.value as Shell)} title="Commande lancée dans le conteneur">
          {SHELLS.map(([id, label]) => (
            <option key={id} value={id}>
              {label}
            </option>
          ))}
        </select>
        {shell === "custom" && (
          <input
            className="input input-sm mono"
            placeholder="/bin/sh -c 'echo bonjour'"
            value={custom}
            onChange={(e) => setCustom(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && canConnect) connect();
            }}
            style={{ width: 220 }}
          />
        )}
        <span className="grow" />
        {busy ? (
          <Spinner label="connexion…" />
        ) : open ? (
          <Badge tone="ok">session ouverte</Badge>
        ) : status === "closed" ? (
          <Badge>session fermée</Badge>
        ) : (
          <Badge>aucune session</Badge>
        )}
        <span className="muted xs nowrap">
          {size.cols}×{size.rows}
          {open && <> · {focused ? "clavier actif" : "cliquer pour saisir"}</>}
        </span>
        {open || busy ? (
          <>
            <button className="btn btn-sm" onClick={() => disconnect()} title="Fermer la session">
              <Plugs size={14} /> Déconnecter
            </button>
            <button className="btn btn-sm btn-icon" onClick={connect} disabled={!canConnect} title="Reconnecter">
              <ArrowsClockwise size={14} />
            </button>
          </>
        ) : (
          <button className="btn btn-sm btn-primary" onClick={connect} disabled={!canConnect} title="Ouvrir une session">
            <PlugsConnected size={14} /> {status === "closed" ? "Reconnecter" : "Connecter"}
          </button>
        )}
        <button
          className="btn btn-sm btn-icon"
          title="Copier la sélection, ou tout le terminal (Ctrl+Maj+C)"
          onClick={() => {
            if (termRef.current) copySelection(termRef.current, true);
          }}
        >
          <Copy size={14} />
        </button>
        <button className="btn btn-sm btn-icon" title="Effacer l'écran" onClick={() => termRef.current?.clear()}>
          <Trash size={14} />
        </button>
      </div>
      {containers.error && <div className="alert alert-err term-alert">{(containers.error as Error).message}</div>}
      {closeMessage && <div className="alert alert-err term-alert">{closeMessage}</div>}
      <div className="term-host">
        <div ref={hostRef} className="term-xterm" />
        {status === "idle" && (
          <div className="term-overlay">
            <TerminalWindow size={36} weight="thin" />
            <div>Choisissez un conteneur puis cliquez sur « Connecter ».</div>
            <button className="btn btn-sm btn-primary" onClick={connect} disabled={!canConnect}>
              <PlugsConnected size={14} /> Connecter
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
