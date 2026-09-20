// Écran Ressources : choix du type, liste triable/filtrable, panneau de détail.
import { ArrowsClockwise, Cube, TerminalWindow } from "@phosphor-icons/react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";
import { useEffect, useMemo, useState } from "react";
import { api } from "@/api/client";
import type { ObjectSummary } from "@/api/types";
import { REFRESH_MS, useKinds } from "@/app/queries";
import { useNamespace, useStore } from "@/app/store";
import { Alert, EmptyState, ReadyBadge, Spinner, StatusBadge } from "@/components/Basics";
import { DataTable } from "@/components/DataTable";
import { age, shortImage } from "@/lib/format";
import { EXTRA_COLUMNS, extraText, FAVOURITE_KINDS, kindLabel, objectKey } from "@/lib/kinds";
import { ResourceDetail } from "./ResourceDetail";
import { YamlConsole } from "./YamlConsole";

const PAGE = 500;

export function ResourcesView() {
  const cluster = useStore((s) => s.cluster);
  const namespace = useNamespace();
  const kindName = useStore((s) => s.resourcesKind);
  const setKind = useStore((s) => s.setResourcesKind);
  const filter = useStore((s) => s.filter);
  const selection = useStore((s) => s.selection);
  const select = useStore((s) => s.select);
  const pendingTarget = useStore((s) => s.pendingTarget);
  const takePending = useStore((s) => s.takePendingTarget);
  const openConsole = useStore((s) => s.openYamlConsole);
  const autoRefresh = useStore((s) => s.autoRefresh);
  const [selector, setSelector] = useState("");
  const qc = useQueryClient();

  const kinds = useKinds(cluster);
  const kind = kinds.data?.find((k) => k.plural === kindName || k.plural + "." + k.group === kindName || k.shortNames.includes(kindName) || k.kind.toLowerCase() === kindName.toLowerCase());
  const namespaced = kind?.namespaced ?? true;

  const list = useQuery({
    queryKey: ["resources", cluster, kindName, namespaced ? namespace : null, selector],
    queryFn: () => api.resources.list(cluster!, kindName, { namespace: namespaced ? namespace : null, labelSelector: selector.trim() || null, limit: PAGE }),
    enabled: !!cluster && !!kindName,
    refetchInterval: autoRefresh ? REFRESH_MS : false,
    placeholderData: (prev) => prev,
  });

  // Navigation demandée par un autre écran : sélectionner l'objet une fois la liste chargée.
  useEffect(() => {
    if (!pendingTarget || !list.data) return;
    if (pendingTarget.name) {
      const found = list.data.items.find((o) => o.name === pendingTarget.name && (pendingTarget.namespace == null || o.namespace === pendingTarget.namespace));
      if (found) select(found);
    }
    takePending();
  }, [pendingTarget, list.data, select, takePending]);

  // Garder la sélection à jour avec les données rafraîchies.
  useEffect(() => {
    if (!selection || !list.data) return;
    const fresh = list.data.items.find((o) => objectKey(o) === objectKey(selection));
    if (fresh && fresh !== selection) select(fresh);
    else if (!fresh && list.data.items.length > 0 && !list.isFetching) select(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [list.data]);

  const sortedKinds = useMemo(() => {
    const all = kinds.data ?? [];
    const fav = FAVOURITE_KINDS.map((p) => all.find((k) => k.plural === p && (k.group === "" || k.group === "apps" || k.group === "batch" || k.group === "networking.k8s.io" || k.group === "autoscaling"))).filter((k): k is NonNullable<typeof k> => !!k);
    const rest = all.filter((k) => !fav.includes(k)).sort((a, b) => kindLabel(a).localeCompare(kindLabel(b)));
    return { fav, rest };
  }, [kinds.data]);

  const showNs = namespaced && namespace == null;
  const columns = useMemo<ColumnDef<ObjectSummary, unknown>[]>(() => {
    const cols: ColumnDef<ObjectSummary, unknown>[] = [
      { id: "name", header: "Nom", accessorKey: "name", cell: (c) => <strong>{c.getValue<string>()}</strong> },
    ];
    if (showNs) cols.push({ id: "namespace", header: "Namespace", accessorFn: (o) => o.namespace ?? "", cell: (c) => <span className="muted">{c.getValue<string>()}</span> });
    if (kindName !== "events") {
      cols.push({ id: "status", header: "Statut", accessorKey: "status", cell: (c) => <StatusBadge status={c.getValue<string>()} /> });
      cols.push({ id: "ready", header: "Prêt", accessorFn: (o) => o.ready ?? "", cell: (c) => <ReadyBadge ready={c.getValue<string>()} /> });
    }
    if (kindName === "pods") {
      cols.push({ id: "restarts", header: "Redém.", accessorFn: (o) => o.restarts ?? 0, meta: { num: true } });
      cols.push({ id: "node", header: "Nœud", accessorFn: (o) => o.node ?? "", cell: (c) => <span className="muted">{c.getValue<string>()}</span> });
    }
    for (const col of EXTRA_COLUMNS[kindName] ?? []) {
      cols.push({ id: "x:" + col.key, header: col.label, accessorFn: (o) => extraText(o.extra[col.key]), meta: { num: col.num }, cell: (c) => <span className={col.key === "message" ? "" : "mono"} style={col.key === "message" ? { whiteSpace: "normal" } : undefined}>{c.getValue<string>()}</span> });
    }
    if (["pods", "deployments", "statefulsets", "daemonsets", "replicasets", "jobs", "cronjobs"].includes(kindName)) {
      cols.push({ id: "images", header: "Images", accessorFn: (o) => o.images.map(shortImage).join(", "), cell: (c) => <span className="muted mono xs" title={c.row.original.images.join("\n")}>{c.getValue<string>()}</span> });
    }
    cols.push({ id: "age", header: "Âge", accessorFn: (o) => o.ageSeconds ?? 0, cell: (c) => <span className="muted">{age(c.getValue<number>())}</span>, meta: { num: true } });
    return cols;
  }, [kindName, showNs]);

  const refresh = () => qc.invalidateQueries({ queryKey: ["resources", cluster] });

  if (!cluster) {
    return (
      <div className="page">
        <EmptyState icon={Cube} title="Aucun cluster sélectionné" hint="Choisissez un cluster dans la barre supérieure." />
      </div>
    );
  }

  return (
    <div className="fill" style={{ flexDirection: "row" }}>
      <div className="fill" style={{ borderRight: selection ? "1px solid var(--border)" : undefined }}>
        <div className="toolbar">
          <select className="select select-sm mono" value={kindName} onChange={(e) => setKind(e.target.value)} style={{ maxWidth: 260 }}>
            {sortedKinds.fav.length > 0 && (
              <optgroup label="Courants">
                {sortedKinds.fav.map((k) => (
                  <option key={kindLabel(k)} value={k.plural}>{k.plural}</option>
                ))}
              </optgroup>
            )}
            <optgroup label="Tous les types">
              {sortedKinds.rest.map((k) => (
                <option key={kindLabel(k)} value={kindLabel(k)}>{kindLabel(k)}</option>
              ))}
            </optgroup>
            {!kinds.data && <option value={kindName}>{kindName}</option>}
          </select>
          <input className="input input-sm mono" placeholder="Sélecteur de labels (app=web)" value={selector} onChange={(e) => setSelector(e.target.value)} style={{ width: 240 }} />
          <span className="muted small">
            {list.data ? `${list.data.items.length} objet${list.data.items.length > 1 ? "s" : ""}${list.data.continueToken ? " (tronqué)" : ""}` : ""}
          </span>
          {list.isFetching && <Spinner />}
          <span className="grow" />
          <button className="btn btn-sm" onClick={() => openConsole()} title="Appliquer, simuler ou comparer un manifeste">
            <TerminalWindow size={14} /> Console YAML
          </button>
          <button className="btn btn-sm btn-icon" onClick={refresh} title="Actualiser">
            <ArrowsClockwise size={14} />
          </button>
        </div>
        {list.error && <div style={{ padding: 8 }}><Alert tone="err">{(list.error as Error).message}</Alert></div>}
        {list.data && (
          <DataTable
            data={list.data.items}
            columns={columns}
            filter={filter}
            rowKey={objectKey}
            selectedKey={selection ? objectKey(selection) : null}
            onRowClick={(o) => select(objectKey(o) === (selection ? objectKey(selection) : "") ? null : o)}
            initialSort={[{ id: "name", desc: false }]}
            empty={<EmptyState icon={Cube} title="Aucun objet" hint={namespace ? `Aucun ${kindName} dans « ${namespace} ».` : `Aucun ${kindName} dans ce cluster.`} />}
          />
        )}
        {list.isLoading && <div className="p-16"><Spinner label="Chargement…" /></div>}
      </div>
      {selection && (
        <div className="fill" style={{ flex: "0 0 46%", maxWidth: 760 }}>
          <ResourceDetail cluster={cluster} obj={selection} kinds={kinds.data} onClose={() => select(null)} onChanged={refresh} />
        </div>
      )}
      <YamlConsole cluster={cluster} onChanged={refresh} />
    </div>
  );
}
