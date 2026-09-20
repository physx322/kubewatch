import { ArrowsClockwise, HardDrives, Plus, Warning } from "@phosphor-icons/react";
import { useMemo } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Alert, Badge, EmptyState, Meter, Spinner } from "@/components/Basics";
import { useClusters, useEvents, useOverview } from "@/app/queries";
import { useNamespace, useStore } from "@/app/store";
import { bytes, millicores, percent, relativeTime } from "@/lib/format";

export function OverviewView() {
  const cluster = useStore((s) => s.cluster);
  const setView = useStore((s) => s.setView);
  const goTo = useStore((s) => s.goToResource);
  const namespace = useNamespace();
  const clusters = useClusters();
  const overview = useOverview(cluster);
  const events = useEvents(cluster, namespace);
  const qc = useQueryClient();

  const info = clusters.data?.find((c) => c.name === cluster);
  const warnings = useMemo(
    () => (events.data ?? []).filter((e) => e.type === "Warning").slice(0, 25),
    [events.data],
  );

  if (!cluster) {
    return (
      <div className="page">
        <EmptyState
          icon={HardDrives}
          title="Aucun cluster sélectionné"
          hint="Importez un kubeconfig ou connectez un serveur d'API pour commencer."
          action={
            <button className="btn btn-primary" onClick={() => setView("clusters")}>
              <Plus size={14} /> Ajouter un cluster
            </button>
          }
        />
      </div>
    );
  }

  const o = overview.data;
  const cpu = o ? percent(o.cpuUsedMillis, o.cpuCapacityMillis) : null;
  const mem = o ? percent(o.memoryUsedBytes, o.memoryCapacityBytes) : null;

  return (
    <div className="page">
      <div className="page-header">
        <h1>{cluster}</h1>
        {info?.version && <Badge tone="info">{info.version}</Badge>}
        {info?.platform && <Badge>{info.platform}</Badge>}
        {info && !info.connected && <Badge tone="err">hors ligne</Badge>}
        <span className="grow" />
        {overview.isFetching && <Spinner />}
        <button
          className="btn btn-sm"
          onClick={() => {
            qc.invalidateQueries({ queryKey: ["overview", cluster] });
            qc.invalidateQueries({ queryKey: ["events", cluster] });
          }}
        >
          <ArrowsClockwise size={14} /> Actualiser
        </button>
      </div>

      {info?.lastError && <Alert tone="err">{info.lastError}</Alert>}
      {overview.error && <Alert tone="err">{(overview.error as Error).message}</Alert>}

      <div className="stats">
        <Stat label="Nœuds prêts" value={o ? `${o.nodesReady} / ${o.nodesTotal}` : "…"} tone={o && o.nodesReady < o.nodesTotal ? "warn" : undefined} />
        <Stat label="Pods en cours" value={o ? `${o.podsRunning} / ${o.podsTotal}` : "…"} />
        <Stat label="Pods en attente" value={o ? String(o.podsPending) : "…"} tone={o && o.podsPending > 0 ? "warn" : undefined} />
        <Stat label="Pods en échec" value={o ? String(o.podsFailed) : "…"} tone={o && o.podsFailed > 0 ? "err" : undefined} />
        <Stat label="Namespaces" value={o ? String(o.namespaces) : "…"} />
      </div>

      <div className="stats">
        <div className="card stat">
          <div className="label">Processeur</div>
          <div className="value">{cpu == null ? "–" : `${cpu.toFixed(0)} %`}</div>
          <div className="sub">
            {o?.metricsAvailable ? `${millicores(o.cpuUsedMillis)} sur ${millicores(o.cpuCapacityMillis)}` : "metrics-server indisponible"}
          </div>
          <Meter value={cpu} />
        </div>
        <div className="card stat">
          <div className="label">Mémoire</div>
          <div className="value">{mem == null ? "–" : `${mem.toFixed(0)} %`}</div>
          <div className="sub">
            {o?.metricsAvailable ? `${bytes(o.memoryUsedBytes)} sur ${bytes(o.memoryCapacityBytes)}` : "metrics-server indisponible"}
          </div>
          <Meter value={mem} />
        </div>
      </div>

      {o && Object.keys(o.workloads).length > 0 && (
        <div className="card">
          <div className="card-header">Charges de travail</div>
          <div className="card-body row wrap">
            {Object.entries(o.workloads).map(([kind, n]) => (
              <button key={kind} className="btn btn-sm" onClick={() => goTo({ kind: kind.toLowerCase() + "s", name: "", namespace: null })}>
                {kind} <Badge>{n}</Badge>
              </button>
            ))}
          </div>
        </div>
      )}

      {o && o.warnings.length > 0 && (
        <Alert tone="warn">
          <Warning size={16} />
          <div className="col gap-4">
            {o.warnings.map((w, i) => (
              <div key={i}>{w}</div>
            ))}
          </div>
        </Alert>
      )}

      <div className="card">
        <div className="card-header">
          <span className="grow">Évènements récents à surveiller</span>
          <span className="muted small">{namespace ?? "tous les namespaces"}</span>
        </div>
        {warnings.length === 0 ? (
          <div className="empty small">Aucun avertissement récent{events.isLoading ? "…" : "."}</div>
        ) : (
          <div className="table-wrap" style={{ maxHeight: 360 }}>
            <table className="table">
              <thead>
                <tr>
                  <th>Quand</th>
                  <th>Objet</th>
                  <th>Raison</th>
                  <th>Message</th>
                  <th>×</th>
                </tr>
              </thead>
              <tbody>
                {warnings.map((e, i) => (
                  <tr
                    key={i}
                    onClick={() =>
                      e.involvedKind && e.involvedName && goTo({ kind: e.involvedKind.toLowerCase() + "s", name: e.involvedName, namespace: e.namespace })
                    }
                  >
                    <td className="muted">{relativeTime(e.lastSeen)}</td>
                    <td className="mono">
                      {e.involvedKind}/{e.namespace ? `${e.namespace}/` : ""}
                      {e.involvedName}
                    </td>
                    <td>
                      <Badge tone="warn">{e.reason}</Badge>
                    </td>
                    <td style={{ whiteSpace: "normal", maxWidth: 600 }}>{e.message}</td>
                    <td className="num">{e.count}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </div>
  );
}

function Stat({ label, value, tone }: { label: string; value: string; tone?: "warn" | "err" }) {
  return (
    <div className="card stat">
      <div className="label">{label}</div>
      <div className="value" style={{ color: tone ? `var(--${tone})` : undefined }}>
        {value}
      </div>
    </div>
  );
}
