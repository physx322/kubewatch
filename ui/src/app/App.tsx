import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { ConfirmHost } from "@/components/Confirm";
import { ErrorBoundary } from "@/components/ErrorBoundary";
import { AssistantPanel } from "@/assistant/AssistantPanel";
import { ClustersView } from "@/views/ClustersView";
import { DeployView } from "@/views/DeployView";
import { HubView } from "@/views/HubView";
import { OverviewView } from "@/views/OverviewView";
import { ResourcesView } from "@/views/ResourcesView";
import { SettingsView } from "@/views/SettingsView";
import { TopologyView } from "@/views/TopologyView";
import { UpdatesView } from "@/views/UpdatesView";
import { useBackendLifecycle } from "./queries";
import { useShortcuts } from "./shortcuts";
import { Sidebar } from "./shell/Sidebar";
import { Toasts } from "./shell/Toasts";
import { Topbar } from "./shell/Topbar";
import { useStore, VIEW_LABELS, type View } from "./store";
import { useApplyTheme } from "./theme";
import "./shell/Shell.css";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { retry: 0, refetchOnWindowFocus: false, staleTime: 2_000 },
  },
});

function CurrentView() {
  const view = useStore((s) => s.view);
  return (
    <ErrorBoundary label={VIEW_LABELS[view]} resetKey={view}>
      <ViewSwitch view={view} />
    </ErrorBoundary>
  );
}

function ViewSwitch({ view }: { view: View }) {
  switch (view) {
    case "overview":
      return <OverviewView />;
    case "resources":
      return <ResourcesView />;
    case "topology":
      return <TopologyView />;
    case "hub":
      return <HubView />;
    case "deploy":
      return <DeployView />;
    case "updates":
      return <UpdatesView />;
    case "clusters":
      return <ClustersView />;
    case "settings":
      return <SettingsView />;
  }
}

function AssistantColumn() {
  const open = useStore((s) => s.assistantOpen);
  const width = useStore((s) => s.assistantWidth);
  const setWidth = useStore((s) => s.setAssistantWidth);
  const dragging = useRef(false);
  const [live, setLive] = useState<number | null>(null);

  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      if (!dragging.current) return;
      setLive(window.innerWidth - e.clientX);
    };
    const onUp = () => {
      if (!dragging.current) return;
      dragging.current = false;
      setLive((w) => {
        if (w != null) setWidth(w);
        return null;
      });
      document.body.style.cursor = "";
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [setWidth]);

  if (!open) return null;
  const w = Math.max(320, Math.min(900, live ?? width));
  return (
    <aside className="assistant-col" style={{ width: w }}>
      <div
        className="assistant-resizer"
        onMouseDown={() => {
          dragging.current = true;
          document.body.style.cursor = "col-resize";
        }}
      />
      <ErrorBoundary label="Assistant" resetKey={open}>
        <AssistantPanel />
      </ErrorBoundary>
    </aside>
  );
}

function Shell() {
  useApplyTheme();
  useShortcuts();
  useBackendLifecycle();
  return (
    <div className="shell">
      <Sidebar />
      <div className="main">
        <Topbar />
        <div className="content">
          <CurrentView />
        </div>
      </div>
      <AssistantColumn />
      <Toasts />
      <ConfirmHost />
    </div>
  );
}

export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <Shell />
    </QueryClientProvider>
  );
}
