// Écran « Mises à jour » : surveillants, détections, inventaire des images et
// historique des déploiements. Port de crates/desktop/src/views/updates.rs ;
// les réglages de l'updater (jeton GitHub, politique par défaut) vivent dans
// l'écran Réglages.
import {
  ArrowSquareOut,
  ArrowsClockwise,
  CaretRight,
  Eye,
  ListChecks,
  MagnifyingGlass,
  Pause,
  Pencil,
  Play,
  Plus,
  Rocket,
  Trash,
} from "@phosphor-icons/react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";
import { useEffect, useMemo, useState } from "react";
import { api } from "@/api/client";
import type { RolloutResult, UpdateFinding, WatcherSpec, WatchersBundle, WorkloadImage } from "@/api/updates";
import { DEFAULT_POLICY, rolloutSucceeded } from "@/api/updates";
import { useNamespaces } from "@/app/queries";
import { useNamespace, useStore } from "@/app/store";
import { Alert, Badge, EmptyState, Spinner } from "@/components/Basics";
import { confirm } from "@/components/Confirm";
import { DataTable } from "@/components/DataTable";
import { Dialog } from "@/components/Dialog";
import { Markdown } from "@/components/Markdown";
import {
  channelLabel,
  newId,
  resourcesKindOf,
  RolloutStatusBadge,
  SeverityBadge,
  sourceLabel,
  SourceTag,
  targetLabel,
} from "@/components/UpdatesCommon";
import { UpdatesWatcherDialog } from "@/components/UpdatesWatcherDialog";
import { count, dateTime, relativeTime, shortImage } from "@/lib/format";
import "./Updates.css";

type Tab = "watchers" | "findings" | "inventory" | "history";

const UPDATES_KEY = ["updates"] as const;
const HISTORY_KEY = ["updates-history"] as const;

interface Editing {
  watcher: WatcherSpec;
  isNew: boolean;
}

export function UpdatesView() {
  const qc = useQueryClient();
  const toast = useStore((s) => s.toast);
  const cluster = useStore((s) => s.cluster);
  const autoRefresh = useStore((s) => s.autoRefresh);
  const [tab, setTab] = useState<Tab>("watchers");
  const [editing, setEditing] = useState<Editing | null>(null);
  const [lastCheck, setLastCheck] = useState<{ at: number; pending: number } | null>(null);

  const bundle = useQuery({
    queryKey: UPDATES_KEY,
    queryFn: api.updates.list,
    // La vérification en tâche de fond peut ajouter des détections : on relit.
    refetchInterval: autoRefresh ? 30_000 : false,
  });
  const settings = useQuery({ queryKey: ["updater-settings"], queryFn: api.updates.settings, staleTime: 60_000 });

  const watchers = bundle.data?.watchers ?? [];
  const findings = bundle.data?.findings ?? [];
  const pendingCount = findings.filter((f) => !f.applied).length;

  const setBundle = (b: WatchersBundle) => qc.setQueryData(UPDATES_KEY, b);

  const check = useMutation({
    mutationFn: api.updates.check,
    onSuccess: (list) => {
      const pending = list.filter((f) => !f.applied).length;
      setLastCheck({ at: Date.now(), pending });
      qc.invalidateQueries({ queryKey: UPDATES_KEY });
      toast(pending > 0 ? "warn" : "ok", pending > 0 ? `${count(pending, "mise à jour disponible", "mises à jour disponibles")}.` : "Aucune nouvelle version détectée.", "Vérification terminée");
      if (pending > 0) setTab("findings");
    },
    onError: (e: Error) => toast("err", e.message, "Vérification impossible"),
  });

  const openNew = (draft?: Partial<WatcherSpec>) => {
    const policy = settings.data?.defaultPolicy ?? DEFAULT_POLICY;
    setEditing({
      isNew: true,
      watcher: {
        id: newId(),
        name: "",
        enabled: true,
        cluster: cluster ?? "",
        target: { namespace: null, kind: "Deployment", name: "" },
        container: null,
        source: { type: "containerRegistry", image: "" },
        policy: { ...policy, ignore: [...policy.ignore] },
        createdAt: new Date().toISOString(),
        lastCheckedAt: null,
        lastKnownVersion: null,
        ...draft,
      },
    });
  };

  const tabs: { id: Tab; label: string; badge?: number; tone?: "warn" | "neutral" }[] = [
    { id: "watchers", label: "Surveillants", badge: watchers.length, tone: "neutral" },
    { id: "findings", label: "Détections", badge: pendingCount, tone: pendingCount > 0 ? "warn" : "neutral" },
    { id: "inventory", label: "Inventaire" },
    { id: "history", label: "Historique" },
  ];

  return (
    <div className="page">
      <div className="page-header">
        <h1>Mises à jour</h1>
        <span className="grow" />
        {bundle.isFetching && !check.isPending && <Spinner />}
        {check.isPending && <Spinner label="vérification en cours…" />}
        <button
          className="btn btn-sm"
          onClick={() => {
            qc.invalidateQueries({ queryKey: UPDATES_KEY });
            qc.invalidateQueries({ queryKey: HISTORY_KEY });
          }}
          title="Relire les surveillants, détections et l'historique"
        >
          <ArrowsClockwise size={14} /> Actualiser
        </button>
        <button
          className="btn btn-primary btn-sm"
          disabled={check.isPending || watchers.length === 0}
          onClick={() => check.mutate()}
          title="Interroge toutes les sources amont des surveillants actifs"
        >
          <MagnifyingGlass size={14} /> Vérifier maintenant
        </button>
      </div>

      {bundle.error && <Alert tone="err">{(bundle.error as Error).message}</Alert>}

      <div className="tabs updates-tabs">
        {tabs.map((t) => (
          <button key={t.id} className={`tab ${tab === t.id ? "active" : ""}`} onClick={() => setTab(t.id)}>
            {t.label}
            {t.badge != null && t.badge > 0 && <Badge tone={t.tone === "warn" ? "warn" : "neutral"}>{t.badge}</Badge>}
          </button>
        ))}
        <span className="grow" />
        <span
          className="updates-note"
          title="Aucune vérification n'a lieu d'elle-même : ni planification externe (cron, webhooks), ni tâche de fond. Cliquez sur « Vérifier maintenant »."
        >
          La vérification part d'un clic : rien ne tourne en tâche de fond.
          {lastCheck && <span> · dernière vérification {relativeTime(new Date(lastCheck.at).toISOString())}</span>}
        </span>
      </div>

      {tab === "watchers" && (
        <WatchersTab
          watchers={watchers}
          loading={bundle.isPending}
          onNew={() => openNew()}
          onEdit={(w) => setEditing({ watcher: w, isNew: false })}
          onBundle={setBundle}
        />
      )}
      {tab === "findings" && <FindingsTab findings={findings} onApplied={() => qc.invalidateQueries({ queryKey: HISTORY_KEY })} />}
      {tab === "inventory" && <InventoryTab onWatch={(draft) => openNew(draft)} onBundle={setBundle} />}
      {tab === "history" && <HistoryTab />}

      <UpdatesWatcherDialog
        open={editing !== null}
        watcher={editing?.watcher ?? null}
        isNew={editing?.isNew ?? true}
        onClose={() => setEditing(null)}
        onSaved={(b) => {
          setBundle(b);
          setEditing(null);
        }}
      />
    </div>
  );
}

// --- Surveillants -------------------------------------------------------------

function WatchersTab({
  watchers,
  loading,
  onNew,
  onEdit,
  onBundle,
}: {
  watchers: WatcherSpec[];
  loading: boolean;
  onNew: () => void;
  onEdit: (w: WatcherSpec) => void;
  onBundle: (b: WatchersBundle) => void;
}) {
  const toast = useStore((s) => s.toast);
  const filter = useStore((s) => s.filter);
  const cluster = useStore((s) => s.cluster);
  const setCluster = useStore((s) => s.setCluster);
  const goToResource = useStore((s) => s.goToResource);

  const toggle = useMutation({
    mutationFn: (w: WatcherSpec) => api.updates.upsertWatcher({ ...w, enabled: !w.enabled }),
    onSuccess: (b, w) => {
      onBundle(b);
      toast("ok", w.enabled ? `« ${w.name} » mis en pause.` : `« ${w.name} » activé.`);
    },
    onError: (e: Error) => toast("err", e.message),
  });

  const remove = async (w: WatcherSpec) => {
    const ok = await confirm({
      title: `Supprimer « ${w.name} » ?`,
      message: "Le surveillant est supprimé, ainsi que ses détections en attente.\nAucun objet du cluster n'est modifié.",
      confirmLabel: "Supprimer",
      danger: true,
    });
    if (!ok) return;
    try {
      onBundle(await api.updates.removeWatcher(w.id));
      toast("ok", `« ${w.name} » supprimé.`);
    } catch (e) {
      toast("err", (e as Error).message);
    }
  };

  const openTarget = (w: WatcherSpec) => {
    if (w.cluster !== cluster) {
      setCluster(w.cluster);
      api.clusters.select(w.cluster).catch(() => {});
    }
    goToResource({ kind: resourcesKindOf(w.target.kind), name: w.target.name, namespace: w.target.namespace ?? null });
  };

  const columns = useMemo<ColumnDef<WatcherSpec, unknown>[]>(
    () => [
      {
        id: "enabled",
        header: "Activé",
        accessorFn: (w) => (w.enabled ? "actif" : "en pause"),
        cell: (c) => (c.row.original.enabled ? <Badge tone="ok">actif</Badge> : <Badge>en pause</Badge>),
        size: 90,
      },
      { id: "name", header: "Nom", accessorKey: "name", cell: (c) => <strong>{c.getValue<string>()}</strong> },
      { id: "cluster", header: "Cluster", accessorKey: "cluster", cell: (c) => <span className="muted">{c.getValue<string>()}</span> },
      {
        id: "target",
        header: "Cible",
        accessorFn: (w) => targetLabel(w.target),
        cell: (c) => {
          const w = c.row.original;
          return (
            <span className="row gap-4">
              <Badge tone="info">{w.target.kind}</Badge>
              <span className="mono">
                {w.target.namespace && <span className="muted">{w.target.namespace}/</span>}
                {w.target.name}
              </span>
              <button
                className="btn btn-ghost btn-sm btn-icon"
                title="Ouvrir dans Ressources"
                onClick={(e) => {
                  e.stopPropagation();
                  openTarget(w);
                }}
              >
                <ArrowSquareOut size={13} />
              </button>
            </span>
          );
        },
      },
      {
        id: "container",
        header: "Conteneur",
        accessorFn: (w) => w.container ?? "",
        cell: (c) => (c.getValue<string>() ? <span className="mono muted">{c.getValue<string>()}</span> : <span className="faint">–</span>),
      },
      { id: "source", header: "Source", accessorFn: (w) => sourceLabel(w.source), cell: (c) => <SourceTag source={c.row.original.source} /> },
      { id: "channel", header: "Canal", accessorFn: (w) => channelLabel(w.policy.channel), cell: (c) => <span>{c.getValue<string>()}</span> },
      {
        id: "lastChecked",
        header: "Dernier contrôle",
        accessorFn: (w) => (w.lastCheckedAt ? Date.parse(w.lastCheckedAt) : 0),
        cell: (c) => {
          const at = c.row.original.lastCheckedAt;
          return at ? (
            <span className="muted" title={dateTime(at)}>
              {relativeTime(at)}
            </span>
          ) : (
            <span className="faint">jamais</span>
          );
        },
      },
      {
        id: "version",
        header: "Version connue",
        accessorFn: (w) => w.lastKnownVersion ?? "",
        cell: (c) => (c.getValue<string>() ? <span className="mono">{c.getValue<string>()}</span> : <span className="faint">–</span>),
      },
      {
        id: "actions",
        header: "",
        enableSorting: false,
        cell: (c) => {
          const w = c.row.original;
          const busy = toggle.isPending && toggle.variables?.id === w.id;
          return (
            <div className="row gap-4" onClick={(e) => e.stopPropagation()}>
              <button className="btn btn-ghost btn-sm btn-icon" title="Modifier" onClick={() => onEdit(w)}>
                <Pencil size={14} />
              </button>
              <button className="btn btn-ghost btn-sm btn-icon" title={w.enabled ? "Mettre en pause" : "Activer"} disabled={busy} onClick={() => toggle.mutate(w)}>
                {busy ? <Spinner /> : w.enabled ? <Pause size={14} /> : <Play size={14} />}
              </button>
              <button className="btn btn-ghost btn-sm btn-icon" title="Supprimer" onClick={() => remove(w)}>
                <Trash size={14} color="var(--err)" />
              </button>
            </div>
          );
        },
        size: 100,
      },
    ],
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [toggle.isPending, toggle.variables, cluster],
  );

  if (loading) {
    return (
      <div className="row muted small">
        <Spinner label="Chargement des surveillants…" />
      </div>
    );
  }

  if (watchers.length === 0) {
    return (
      <EmptyState
        icon={Eye}
        title="Aucun surveillant"
        hint="Déclarez un surveillant à la main, ou laissez l'onglet Inventaire en proposer à partir des images déployées."
        action={
          <button className="btn btn-primary" onClick={onNew}>
            <Plus size={14} /> Nouveau surveillant
          </button>
        }
      />
    );
  }

  return (
    <div className="panel">
      <div className="toolbar">
        <span className="small muted">
          {count(watchers.length, "surveillant")} · {count(watchers.filter((w) => w.enabled).length, "actif")}
        </span>
        <span className="grow" />
        <button className="btn btn-primary btn-sm" onClick={onNew}>
          <Plus size={14} /> Nouveau surveillant
        </button>
      </div>
      <DataTable
        data={watchers}
        columns={columns}
        filter={filter}
        rowKey={(w) => w.id}
        onRowClick={onEdit}
        initialSort={[{ id: "name", desc: false }]}
        empty={<div className="empty small">Aucun surveillant ne correspond au filtre.</div>}
      />
    </div>
  );
}

// --- Détections ------------------------------------------------------------------

function FindingsTab({ findings, onApplied }: { findings: UpdateFinding[]; onApplied: () => void }) {
  const qc = useQueryClient();
  const toast = useStore((s) => s.toast);
  const filter = useStore((s) => s.filter);
  const [showApplied, setShowApplied] = useState(false);
  const [result, setResult] = useState<RolloutResult | null>(null);

  const apply = useMutation({
    mutationFn: (f: UpdateFinding) => api.updates.apply(f.id),
    onSuccess: (r) => {
      setResult(r);
      qc.invalidateQueries({ queryKey: UPDATES_KEY });
      onApplied();
      if (rolloutSucceeded(r)) toast("ok", `${targetLabel(r.target)} → ${shortImage(r.newImage)}`, "Mise à jour appliquée");
      else toast("err", r.status, "Mise à jour non appliquée");
    },
    onError: (e: Error) => toast("err", e.message, "Application impossible"),
  });

  const ask = async (f: UpdateFinding) => {
    const image = f.availableImage ?? f.availableVersion;
    const ok = await confirm({
      title: "Appliquer la mise à jour ?",
      message: `Cible : ${f.cluster} · ${findingTarget(f)}\nImage déployée : ${image}\n\nLe déploiement est mis à jour immédiatement.`,
      confirmLabel: "Appliquer",
      danger: true,
    });
    if (ok) apply.mutate(f);
  };

  const appliedCount = findings.filter((f) => f.applied).length;
  const q = filter.trim().toLowerCase();
  const visible = findings
    .filter((f) => showApplied || !f.applied)
    .filter((f) =>
      q ? `${f.watcherName} ${f.cluster} ${findingTarget(f)} ${f.currentVersion ?? ""} ${f.availableVersion} ${f.availableImage ?? ""} ${sourceLabel(f.source)}`.toLowerCase().includes(q) : true,
    )
    .sort((a, b) => Date.parse(b.detectedAt) - Date.parse(a.detectedAt));

  return (
    <div className="col gap-12">
      <div className="updates-toolbar">
        <span className="small muted">
          {findings.length - appliedCount === 0 ? "Aucune mise à jour en attente." : `${count(findings.length - appliedCount, "mise à jour en attente", "mises à jour en attente")}.`}
        </span>
        {apply.isPending && <Spinner label="application en cours…" />}
        <span className="grow" />
        {appliedCount > 0 && (
          <label className="checkbox small">
            <input type="checkbox" checked={showApplied} onChange={(e) => setShowApplied(e.target.checked)} />
            Afficher les {count(appliedCount, "détection déjà appliquée", "détections déjà appliquées")}
          </label>
        )}
      </div>

      {visible.length === 0 ? (
        <EmptyState
          icon={ListChecks}
          title={findings.length === 0 ? "Rien à signaler" : "Aucune détection ne correspond au filtre"}
          hint={findings.length === 0 ? "Aucune version plus récente n'a été détectée. Lancez « Vérifier maintenant » pour interroger les sources." : undefined}
        />
      ) : (
        <div className="findings">
          {visible.map((f) => (
            <FindingCard key={f.id} finding={f} busy={apply.isPending} applying={apply.isPending && apply.variables?.id === f.id} onApply={() => ask(f)} />
          ))}
        </div>
      )}

      <RolloutDialog result={result} onClose={() => setResult(null)} />
    </div>
  );
}

function findingTarget(f: UpdateFinding): string {
  return targetLabel({ namespace: f.targetNamespace ?? null, kind: f.targetKind, name: f.targetName });
}

function FindingCard({ finding: f, busy, applying, onApply }: { finding: UpdateFinding; busy: boolean; applying: boolean; onApply: () => void }) {
  const toast = useStore((s) => s.toast);
  const release = f.release;
  const notes = release?.body?.trim() ? release.body : null;
  const current = f.currentVersion ?? f.currentImage ?? null;
  return (
    <div className={`finding ${f.applied ? "applied" : ""}`}>
      <div className="finding-head">
        <SeverityBadge severity={f.severity} />
        <strong>{f.watcherName}</strong>
        <span className="muted small">{f.cluster}</span>
        <span className="muted small mono">{findingTarget(f)}</span>
        {f.container && <span className="muted small">conteneur {f.container}</span>}
        <span className="grow" />
        <span className="faint xs" title={dateTime(f.detectedAt)}>
          détectée {relativeTime(f.detectedAt)}
        </span>
      </div>
      <div className="finding-versions">
        {current ? <span>{current}</span> : <span className="faint">version inconnue</span>}
        <span className="arrow">→</span>
        <strong>{f.availableVersion}</strong>
        {release?.prerelease && <Badge tone="warn">pré-version</Badge>}
        {release?.publishedAt && <span className="muted xs">publiée le {dateTime(release.publishedAt)}</span>}
      </div>
      {f.availableImage && (
        <div className="finding-image" title="Image complète à déployer">
          {f.availableImage}
        </div>
      )}
      {notes && (
        <details className="finding-notes">
          <summary>
            <CaretRight size={12} /> Notes de version — {release?.name || release?.tag}
          </summary>
          <div className="finding-notes-body">
            <Markdown text={notes} />
          </div>
        </details>
      )}
      <div className="finding-actions">
        {f.applied ? (
          <Badge tone="ok">déjà appliquée</Badge>
        ) : (
          <button className="btn btn-primary btn-sm" disabled={busy} onClick={onApply}>
            {applying ? <Spinner /> : <Rocket size={14} />} Appliquer
          </button>
        )}
        {release?.htmlUrl && (
          <button className="btn btn-ghost btn-sm" onClick={() => api.openUrl(release.htmlUrl!).catch((e: Error) => toast("err", e.message))}>
            <ArrowSquareOut size={13} /> Page de la release
          </button>
        )}
        <span className="grow" />
        <SourceTag source={f.source} />
      </div>
    </div>
  );
}

function RolloutDialog({ result, onClose }: { result: RolloutResult | null; onClose: () => void }) {
  return (
    <Dialog open={!!result} title="Résultat du déploiement" onClose={onClose} icon={<Rocket size={20} />} footer={<button className="btn btn-primary" onClick={onClose}>Fermer</button>}>
      {result && (
        <>
          {rolloutSucceeded(result) ? (
            <Alert tone="ok">La mise à jour a été appliquée. Le déploiement converge vers la nouvelle image.</Alert>
          ) : (
            <Alert tone="err">Le déploiement a échoué : {result.status.replace(/^failed:?\s*/, "") || "raison inconnue"}.</Alert>
          )}
          <div className="rollout-kv">
            <span className="k">Cible</span>
            <span className="v mono">{targetLabel(result.target)}</span>
            <span className="k">Image précédente</span>
            <span className="v mono">{result.previousImage ?? <span className="faint">–</span>}</span>
            <span className="k">Nouvelle image</span>
            <span className="v mono">{result.newImage}</span>
            <span className="k">Appliquée le</span>
            <span className="v">{dateTime(result.appliedAt)}</span>
            <span className="k">Statut</span>
            <span className="v">
              <RolloutStatusBadge result={result} />
            </span>
          </div>
        </>
      )}
    </Dialog>
  );
}

// --- Inventaire -----------------------------------------------------------------

function InventoryTab({ onWatch, onBundle }: { onWatch: (draft: Partial<WatcherSpec>) => void; onBundle: (b: WatchersBundle) => void }) {
  const qc = useQueryClient();
  const toast = useStore((s) => s.toast);
  const filter = useStore((s) => s.filter);
  const cluster = useStore((s) => s.cluster);
  const globalNs = useNamespace();
  const namespaces = useNamespaces(cluster);
  const [namespace, setNamespace] = useState<string | null>(globalNs);
  const [suggestions, setSuggestions] = useState<WatcherSpec[]>([]);
  const [checked, setChecked] = useState<Record<string, boolean>>({});

  // Le namespace de la barre supérieure sert de point de départ ; le cluster
  // courant s'impose (un scan porte sur le cluster sélectionné).
  useEffect(() => setNamespace(globalNs), [globalNs, cluster]);

  const scan = useMutation({
    mutationFn: () => api.updates.scan(cluster!, namespace),
    onError: (e: Error) => toast("err", e.message, "Inventaire impossible"),
  });
  const suggest = useMutation({
    mutationFn: () => api.updates.suggest(cluster!, namespace),
    onSuccess: (list) => {
      setSuggestions(list);
      setChecked(Object.fromEntries(list.map((w) => [w.id, true])));
      if (list.length === 0) toast("info", "Aucune charge de travail à surveiller dans ce périmètre.");
    },
    onError: (e: Error) => toast("err", e.message, "Suggestion impossible"),
  });
  const saveSelected = useMutation({
    mutationFn: async () => {
      const selected = suggestions.filter((w) => checked[w.id]);
      let last: WatchersBundle | null = null;
      const errors: string[] = [];
      for (const w of selected) {
        try {
          last = await api.updates.upsertWatcher(w);
        } catch (e) {
          errors.push(`${w.name} : ${(e as Error).message}`);
        }
      }
      return { saved: selected.length - errors.length, errors, last };
    },
    onSuccess: ({ saved, errors, last }) => {
      if (last) onBundle(last);
      else qc.invalidateQueries({ queryKey: UPDATES_KEY });
      if (saved > 0) toast("ok", `${count(saved, "surveillant enregistré", "surveillants enregistrés")}.`);
      for (const e of errors) toast("err", e, "Enregistrement impossible");
      if (errors.length === 0) {
        setSuggestions([]);
        setChecked({});
      }
    },
  });

  useEffect(() => {
    scan.reset();
    setSuggestions([]);
    setChecked({});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cluster, namespace]);

  const images = scan.data ?? [];
  const columns = useMemo<ColumnDef<WorkloadImage, unknown>[]>(
    () => [
      { id: "namespace", header: "Namespace", accessorFn: (w) => w.namespace ?? "", cell: (c) => (c.getValue<string>() ? <span className="muted">{c.getValue<string>()}</span> : <span className="faint">–</span>) },
      { id: "kind", header: "Type", accessorKey: "kind", cell: (c) => <Badge tone="info">{c.getValue<string>()}</Badge> },
      { id: "name", header: "Nom", accessorKey: "name", cell: (c) => <strong>{c.getValue<string>()}</strong> },
      { id: "container", header: "Conteneur", accessorKey: "container", cell: (c) => <span className="mono muted">{c.getValue<string>()}</span> },
      {
        id: "image",
        header: "Image",
        accessorKey: "image",
        cell: (c) => (
          <span className="mono selectable" title={c.getValue<string>()}>
            {shortImage(c.getValue<string>())}
          </span>
        ),
      },
      {
        id: "repo",
        header: "Dépôt source probable",
        accessorFn: (w) => w.sourceRepoGuess ?? "",
        cell: (c) => (c.getValue<string>() ? <span className="tag">github:{c.getValue<string>()}</span> : <span className="faint">–</span>),
      },
      {
        id: "actions",
        header: "",
        enableSorting: false,
        cell: (c) => {
          const w = c.row.original;
          return (
            <div onClick={(e) => e.stopPropagation()}>
              <button className="btn btn-sm" title="Créer un surveillant pour cette image" onClick={() => onWatch(draftFromImage(w))}>
                <Eye size={13} /> Surveiller
              </button>
            </div>
          );
        },
        size: 110,
      },
    ],
    [onWatch],
  );

  if (!cluster) {
    return <Alert tone="warn">Sélectionnez un cluster dans la barre supérieure pour explorer ses charges de travail.</Alert>;
  }

  const selectedCount = suggestions.filter((w) => checked[w.id]).length;
  const q = filter.trim().toLowerCase();
  const visibleSuggestions = suggestions.filter((w) => (q ? `${w.name} ${targetLabel(w.target)} ${sourceLabel(w.source)}`.toLowerCase().includes(q) : true));

  return (
    <div className="col gap-12">
      <div className="updates-toolbar">
        <span className="small muted">Cluster « {cluster} »</span>
        <select className="select select-sm" value={namespace ?? "*"} onChange={(e) => setNamespace(e.target.value === "*" ? null : e.target.value)} title="Namespace inventorié">
          <option value="*">Tous les namespaces</option>
          {(namespaces.data ?? []).map((ns) => (
            <option key={ns} value={ns}>
              {ns}
            </option>
          ))}
        </select>
        <button className="btn btn-sm" disabled={scan.isPending} onClick={() => scan.mutate()}>
          {scan.isPending ? <Spinner /> : <MagnifyingGlass size={14} />} Inventorier les images
        </button>
        <button className="btn btn-primary btn-sm" disabled={suggest.isPending} onClick={() => suggest.mutate()}>
          {suggest.isPending ? <Spinner /> : <ListChecks size={14} />} Suggérer des surveillants
        </button>
      </div>

      {suggestions.length > 0 && (
        <div className="panel">
          <div className="toolbar">
            <strong className="small">{count(suggestions.length, "surveillant proposé", "surveillants proposés")}</strong>
            <span className="muted xs">Source GitHub quand le dépôt amont est connu, sinon le registre d'images.</span>
            <span className="grow" />
            <button className="btn btn-ghost btn-sm" onClick={() => setChecked(Object.fromEntries(suggestions.map((w) => [w.id, true])))}>
              Tout
            </button>
            <button className="btn btn-ghost btn-sm" onClick={() => setChecked({})}>
              Aucun
            </button>
            <button className="btn btn-ghost btn-sm" onClick={() => { setSuggestions([]); setChecked({}); }}>
              Ignorer
            </button>
            <button className="btn btn-primary btn-sm" disabled={selectedCount === 0 || saveSelected.isPending} onClick={() => saveSelected.mutate()}>
              {saveSelected.isPending ? <Spinner /> : <Plus size={14} />} Enregistrer la sélection{selectedCount > 0 ? ` (${selectedCount})` : ""}
            </button>
          </div>
          <div className="suggestions">
            {visibleSuggestions.map((w) => (
              <label key={w.id} className="suggestion">
                <input type="checkbox" checked={!!checked[w.id]} onChange={(e) => setChecked({ ...checked, [w.id]: e.target.checked })} />
                <span className="grow truncate">
                  <strong>{w.name}</strong> <span className="muted mono xs">{targetLabel(w.target)}</span>
                </span>
                {w.lastKnownVersion && (
                  <span className="mono xs muted" title="Version actuellement déployée">
                    {w.lastKnownVersion}
                  </span>
                )}
                <SourceTag source={w.source} />
              </label>
            ))}
            {visibleSuggestions.length === 0 && <div className="empty small">Aucune suggestion ne correspond au filtre.</div>}
          </div>
        </div>
      )}

      {scan.data ? (
        images.length === 0 ? (
          <EmptyState icon={MagnifyingGlass} title="Aucune image" hint="Aucun Deployment, StatefulSet, DaemonSet ni CronJob dans ce périmètre." />
        ) : (
          <div className="panel">
            <div className="toolbar">
              <span className="small muted">{count(images.length, "image déployée", "images déployées")}</span>
            </div>
            <DataTable data={images} columns={columns} filter={filter} rowKey={(w) => `${w.namespace ?? ""}/${w.kind}/${w.name}/${w.container}`} onRowClick={(w) => onWatch(draftFromImage(w))} empty={<div className="empty small">Aucune image ne correspond au filtre.</div>} />
          </div>
        )
      ) : (
        !scan.isPending &&
        suggestions.length === 0 && (
          <EmptyState icon={MagnifyingGlass} title="Inventaire non lancé" hint="Inventoriez les images déployées, ou demandez directement des suggestions de surveillants." />
        )
      )}
    </div>
  );
}

/** Brouillon de surveillant à partir d'une image inventoriée (même logique que `suggest_watchers`). */
function draftFromImage(w: WorkloadImage): Partial<WatcherSpec> {
  const gh = w.sourceRepoGuess?.split("/") ?? [];
  const source: WatcherSpec["source"] =
    gh.length === 2 && gh[0] && gh[1] ? { type: "githubRelease", owner: gh[0], repo: gh[1], tagPrefix: null } : { type: "containerRegistry", image: w.image };
  return {
    name: `${w.kind} ${w.name}/${w.container}`,
    cluster: w.cluster,
    target: { namespace: w.namespace, kind: w.kind, name: w.name },
    container: w.container,
    source,
  };
}

// --- Historique -------------------------------------------------------------------

function HistoryTab() {
  const filter = useStore((s) => s.filter);
  const history = useQuery({ queryKey: HISTORY_KEY, queryFn: api.updates.history });
  const rows = history.data ?? [];

  const columns = useMemo<ColumnDef<RolloutResult, unknown>[]>(
    () => [
      {
        id: "appliedAt",
        header: "Date",
        accessorFn: (r) => Date.parse(r.appliedAt) || 0,
        cell: (c) => <span title={relativeTime(c.row.original.appliedAt)}>{dateTime(c.row.original.appliedAt)}</span>,
      },
      { id: "target", header: "Cible", accessorFn: (r) => targetLabel(r.target), cell: (c) => <span className="mono">{c.getValue<string>()}</span> },
      {
        id: "previous",
        header: "Image précédente",
        accessorFn: (r) => r.previousImage ?? "",
        cell: (c) => (c.getValue<string>() ? <span className="mono muted selectable" title={c.getValue<string>()}>{shortImage(c.getValue<string>())}</span> : <span className="faint">–</span>),
      },
      {
        id: "new",
        header: "Nouvelle image",
        accessorKey: "newImage",
        cell: (c) => (
          <span className="mono selectable" title={c.getValue<string>()}>
            {shortImage(c.getValue<string>())}
          </span>
        ),
      },
      { id: "status", header: "Statut", accessorKey: "status", cell: (c) => <RolloutStatusBadge result={c.row.original} /> },
    ],
    [],
  );

  return (
    <div className="col gap-12">
      <div className="updates-toolbar">
        <span className="small muted">Déploiements de mise à jour effectués par KubeWatch (200 derniers).</span>
        {history.isFetching && <Spinner />}
        <span className="grow" />
        <button className="btn btn-sm" onClick={() => history.refetch()}>
          <ArrowsClockwise size={14} /> Recharger
        </button>
      </div>
      {history.error && <Alert tone="err">{(history.error as Error).message}</Alert>}
      {history.isPending ? (
        <Spinner label="Chargement de l'historique…" />
      ) : rows.length === 0 ? (
        <EmptyState icon={Rocket} title="Aucun déploiement enregistré" hint="Les mises à jour appliquées depuis l'onglet Détections (ou automatiquement) apparaîtront ici." />
      ) : (
        <div className="panel">
          <DataTable
            data={rows}
            columns={columns}
            filter={filter}
            rowKey={(r) => `${r.appliedAt}|${r.findingId}|${r.newImage}`}
            initialSort={[{ id: "appliedAt", desc: true }]}
            empty={<div className="empty small">Aucun déploiement ne correspond au filtre.</div>}
          />
        </div>
      )}
    </div>
  );
}
