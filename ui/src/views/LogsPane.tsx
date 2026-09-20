// Journaux d'un pod, en flux, avec conteneur, suivi, filtre et défilement automatique.
import { ArrowDown, Copy, Pause, Play, Trash } from "@phosphor-icons/react";
import { useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "@/api/client";
import type { ResourceRef } from "@/api/types";
import { notify } from "@/app/store";
import { Spinner } from "@/components/Basics";

const MAX_LINES = 20_000;

export function LogsPane({ cluster, pod, onLinesChange }: { cluster: string; pod: ResourceRef; onLinesChange?: (tail: string) => void }) {
  const containers = useQuery({
    queryKey: ["containers", cluster, pod.namespace, pod.name],
    queryFn: () => api.resources.containers(cluster, pod),
  });
  const [container, setContainer] = useState<string>("");
  const [follow, setFollow] = useState(true);
  const [tail, setTail] = useState(500);
  const [timestamps, setTimestamps] = useState(false);
  const [previous, setPrevious] = useState(false);
  const [filter, setFilter] = useState("");
  const [lines, setLines] = useState<string[]>([]);
  const [status, setStatus] = useState<"idle" | "streaming" | "ended" | "error">("idle");
  const [error, setError] = useState<string | null>(null);
  const [stick, setStick] = useState(true);
  const streamId = useRef<number | null>(null);
  const preRef = useRef<HTMLPreElement>(null);
  const pending = useRef<string[]>([]);
  const flushTimer = useRef<number | null>(null);

  useEffect(() => {
    if (containers.data && !container) {
      const main = containers.data.find((c) => !c.init) ?? containers.data[0];
      if (main) setContainer(main.name);
    }
  }, [containers.data, container]);

  const stop = useCallback(async () => {
    if (streamId.current != null) {
      const id = streamId.current;
      streamId.current = null;
      await api.logs.stop(id).catch(() => {});
    }
  }, []);

  const flush = useCallback(() => {
    flushTimer.current = null;
    if (pending.current.length === 0) return;
    const add = pending.current;
    pending.current = [];
    setLines((prev) => {
      const next = prev.length + add.length > MAX_LINES ? [...prev.slice(prev.length + add.length - MAX_LINES), ...add] : [...prev, ...add];
      return next;
    });
  }, []);

  const start = useCallback(async () => {
    await stop();
    setLines([]);
    pending.current = [];
    setError(null);
    setStatus("streaming");
    try {
      const id = await api.logs.start(
        cluster,
        pod,
        { container: container || null, follow, tailLines: tail, timestamps, previous },
        (ev) => {
          if (ev.type === "lines") {
            pending.current.push(...ev.lines);
            if (flushTimer.current == null) flushTimer.current = window.setTimeout(flush, 50);
          } else {
            flush();
            setStatus(ev.error ? "error" : "ended");
            if (ev.error) setError(ev.error);
            streamId.current = null;
          }
        },
      );
      streamId.current = id;
    } catch (e) {
      setStatus("error");
      setError((e as Error).message);
    }
  }, [cluster, pod, container, follow, tail, timestamps, previous, stop, flush]);

  // Démarrage automatique dès que le conteneur est connu ; arrêt au démontage.
  useEffect(() => {
    if (!container) return;
    start();
    return () => {
      stop();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [container, previous, timestamps, follow, tail]);

  useEffect(() => {
    if (stick && preRef.current) preRef.current.scrollTop = preRef.current.scrollHeight;
  }, [lines, stick]);

  useEffect(() => {
    if (onLinesChange) onLinesChange(lines.slice(-200).join("\n"));
  }, [lines, onLinesChange]);

  const shown = useMemo(() => (filter ? lines.filter((l) => l.toLowerCase().includes(filter.toLowerCase())) : lines), [lines, filter]);

  return (
    <div className="fill">
      <div className="toolbar" style={{ flexWrap: "wrap" }}>
        <select className="select select-sm" value={container} onChange={(e) => setContainer(e.target.value)} title="Conteneur">
          {containers.data?.map((c) => (
            <option key={c.name} value={c.name}>
              {c.init ? "init: " : ""}
              {c.name}
            </option>
          ))}
        </select>
        <label className="checkbox small">
          <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} /> Suivre
        </label>
        <label className="checkbox small">
          <input type="checkbox" checked={timestamps} onChange={(e) => setTimestamps(e.target.checked)} /> Horodatage
        </label>
        <label className="checkbox small">
          <input type="checkbox" checked={previous} onChange={(e) => setPrevious(e.target.checked)} /> Instance précédente
        </label>
        <select className="select select-sm" value={tail} onChange={(e) => setTail(Number(e.target.value))} title="Lignes depuis la fin">
          {[100, 500, 2000, 10000].map((n) => (
            <option key={n} value={n}>
              {n} lignes
            </option>
          ))}
        </select>
        <input className="input input-sm" placeholder="Filtrer les lignes…" value={filter} onChange={(e) => setFilter(e.target.value)} style={{ width: 180 }} />
        <span className="grow" />
        <span className="muted xs">
          {status === "streaming" ? <Spinner label={follow ? "en direct" : "lecture"} /> : status === "ended" ? "terminé" : status === "error" ? "interrompu" : ""} · {shown.length} ligne{shown.length > 1 ? "s" : ""}
        </span>
        {status === "streaming" ? (
          <button className="btn btn-sm btn-icon" title="Arrêter" onClick={() => stop().then(() => setStatus("ended"))}>
            <Pause size={14} />
          </button>
        ) : (
          <button className="btn btn-sm btn-icon" title="Relancer" onClick={start}>
            <Play size={14} />
          </button>
        )}
        <button className={`btn btn-sm btn-icon ${stick ? "btn-primary" : ""}`} title="Défilement automatique" onClick={() => setStick(!stick)}>
          <ArrowDown size={14} />
        </button>
        <button className="btn btn-sm btn-icon" title="Copier" onClick={() => navigator.clipboard.writeText(shown.join("\n")).then(() => notify.ok("Journal copié."))}>
          <Copy size={14} />
        </button>
        <button className="btn btn-sm btn-icon" title="Effacer" onClick={() => setLines([])}>
          <Trash size={14} />
        </button>
      </div>
      {error && <div className="alert alert-err" style={{ margin: 8 }}>{error}</div>}
      <pre
        ref={preRef}
        className="log fill"
        onScroll={(e) => {
          const el = e.currentTarget;
          setStick(el.scrollHeight - el.scrollTop - el.clientHeight < 24);
        }}
      >
        {shown.join("\n")}
      </pre>
    </div>
  );
}
