import {
  ArrowsClockwise,
  Cube,
  Gear,
  Graph,
  HardDrives,
  RocketLaunch,
  SquaresFour,
  Storefront,
  type Icon,
} from "@phosphor-icons/react";
import { useQuery } from "@tanstack/react-query";
import { api } from "@/api/client";
import { useStore, VIEW_LABELS, type View } from "../store";

const MAIN: { id: View; icon: Icon; key: string }[] = [
  { id: "overview", icon: SquaresFour, key: "1" },
  { id: "resources", icon: Cube, key: "2" },
  { id: "topology", icon: Graph, key: "3" },
  { id: "hub", icon: Storefront, key: "4" },
  { id: "deploy", icon: RocketLaunch, key: "5" },
  { id: "updates", icon: ArrowsClockwise, key: "6" },
];

const SYSTEM: { id: View; icon: Icon; key: string }[] = [
  { id: "clusters", icon: HardDrives, key: "7" },
  { id: "settings", icon: Gear, key: "," },
];

export function Sidebar() {
  const view = useStore((s) => s.view);
  const setView = useStore((s) => s.setView);
  const info = useQuery({ queryKey: ["app-info-static"], queryFn: api.appInfo, staleTime: Infinity });

  const item = ({ id, icon: I, key }: { id: View; icon: Icon; key: string }) => (
    <button
      key={id}
      className={`nav-item ${view === id ? "active" : ""}`}
      onClick={() => setView(id)}
      title={`${VIEW_LABELS[id]} (Ctrl+${key})`}
    >
      <I size={18} weight={view === id ? "fill" : "regular"} />
      {VIEW_LABELS[id]}
      <span className="shortcut">⌃{key}</span>
    </button>
  );

  return (
    <nav className="sidebar">
      <div className="sidebar-brand">
        <img src="/icon.svg" alt="" />
        <div className="col" style={{ gap: 0 }}>
          <strong>KubeWatch</strong>
          <span className="xs">Kubernetes, sur le bureau</span>
        </div>
      </div>
      {MAIN.map(item)}
      <div className="sidebar-spacer" />
      <div className="sidebar-section">Système</div>
      {SYSTEM.map(item)}
      <div className="sidebar-footer">
        <span>v{info.data?.version ?? "…"}</span>
      </div>
    </nav>
  );
}
