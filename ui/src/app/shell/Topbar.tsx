import {
  ArrowsClockwise,
  MagnifyingGlass,
  Moon,
  Sparkle,
  Sun,
  SunHorizon,
} from "@phosphor-icons/react";
import { useEffect, useRef } from "react";
import { api } from "@/api/client";
import { useClusters, useNamespaces } from "../queries";
import { useNamespace, useStore, VIEW_LABELS } from "../store";
import { Dot } from "@/components/Basics";

const THEME_NEXT = { auto: "light", light: "dark", dark: "auto" } as const;
const THEME_LABEL = { auto: "Thème : système", light: "Thème : clair", dark: "Thème : sombre" } as const;

export function Topbar() {
  const view = useStore((s) => s.view);
  const cluster = useStore((s) => s.cluster);
  const setCluster = useStore((s) => s.setCluster);
  const namespace = useNamespace();
  const setNamespace = useStore((s) => s.setNamespace);
  const filter = useStore((s) => s.filter);
  const setFilter = useStore((s) => s.setFilter);
  const searchTick = useStore((s) => s.searchTick);
  const autoRefresh = useStore((s) => s.autoRefresh);
  const toggleAutoRefresh = useStore((s) => s.toggleAutoRefresh);
  const theme = useStore((s) => s.theme);
  const setTheme = useStore((s) => s.setTheme);
  const assistantOpen = useStore((s) => s.assistantOpen);
  const toggleAssistant = useStore((s) => s.toggleAssistant);
  const toast = useStore((s) => s.toast);

  const clusters = useClusters();
  const namespaces = useNamespaces(cluster);
  const searchRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (searchTick > 0) searchRef.current?.focus();
  }, [searchTick]);

  const current = clusters.data?.find((c) => c.name === cluster);
  const tone = !current ? "neutral" : current.connected ? "ok" : "err";
  const ThemeIcon = theme === "dark" ? Moon : theme === "light" ? Sun : SunHorizon;

  return (
    <header className="topbar">
      <span className="topbar-title">{VIEW_LABELS[view]}</span>

      <label className="cluster-pill" title={current?.server ?? "Aucun cluster"}>
        <Dot tone={tone} />
        <select
          className="select select-sm"
          value={cluster ?? ""}
          onChange={(e) => {
            const name = e.target.value || null;
            setCluster(name);
            if (name) api.clusters.select(name).catch((err: Error) => toast("err", err.message));
          }}
        >
          <option value="">Aucun cluster</option>
          {clusters.data?.map((c) => (
            <option key={c.name} value={c.name}>
              {c.name}
              {c.connected ? "" : " (hors ligne)"}
            </option>
          ))}
        </select>
      </label>

      <select
        className="select select-sm"
        value={namespace ?? "*"}
        disabled={!cluster}
        onChange={(e) => setNamespace(e.target.value === "*" ? null : e.target.value)}
        title="Namespace"
      >
        <option value="*">Tous les namespaces</option>
        {namespaces.data?.map((ns) => (
          <option key={ns} value={ns}>
            {ns}
          </option>
        ))}
      </select>

      <span className="grow" />

      <div className="search">
        <MagnifyingGlass size={14} />
        <input
          ref={searchRef}
          className="input input-sm"
          placeholder="Filtrer… (Ctrl+K)"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          onKeyDown={(e) => e.key === "Escape" && setFilter("")}
        />
      </div>

      <button
        className={`btn btn-ghost btn-icon ${autoRefresh ? "active" : ""}`}
        onClick={toggleAutoRefresh}
        title={autoRefresh ? "Actualisation automatique : activée" : "Actualisation automatique : coupée"}
        style={{ color: autoRefresh ? "var(--accent)" : undefined }}
      >
        <ArrowsClockwise size={17} />
      </button>
      <button className="btn btn-ghost btn-icon" onClick={() => setTheme(THEME_NEXT[theme])} title={THEME_LABEL[theme]}>
        <ThemeIcon size={17} />
      </button>
      <button
        className={`btn btn-icon ${assistantOpen ? "btn-primary" : ""}`}
        onClick={toggleAssistant}
        title="Assistant IA (Ctrl+J)"
      >
        <Sparkle size={17} weight={assistantOpen ? "fill" : "regular"} />
      </button>
    </header>
  );
}
