// Panneau de détail d'un objet : résumé, YAML, évènements, journaux, terminal, actions.
import {
  ArrowsClockwise,
  CaretDown,
  Copy,
  FloppyDisk,
  Sparkle,
  Trash,
  X,
} from "@phosphor-icons/react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import { api } from "@/api/client";
import type { ObjectSummary, ResourceKind, ResourceRef } from "@/api/types";
import { useStore } from "@/app/store";
import { useAssistant } from "@/assistant/store";
import { Alert, Badge, Field, ReadyBadge, Spinner, StatusBadge } from "@/components/Basics";
import { confirm } from "@/components/Confirm";
import { Dialog } from "@/components/Dialog";
import { ContextMenu, type MenuItem } from "@/components/Menu";
import { TerminalPane } from "@/components/TerminalPane";
import { YamlEditor } from "@/components/YamlEditor";
import { age, dateTime, relativeTime, shortImage } from "@/lib/format";
import { extraText, refOf, ROLLBACK_KINDS, SCALABLE_KINDS, WORKLOAD_KINDS } from "@/lib/kinds";
import { LogsPane } from "./LogsPane";

type Tab = "summary" | "yaml" | "events" | "logs" | "terminal";

export function ResourceDetail({
  cluster,
  obj,
  kinds,
  onClose,
  onChanged,
}: {
  cluster: string;
  obj: ObjectSummary;
  kinds: ResourceKind[] | undefined;
  onClose: () => void;
  onChanged: () => void;
}) {
  const ref = useMemo(() => refOf(obj, kinds), [obj, kinds]);
  const isPod = obj.kind === "Pod";
  const [tab, setTab] = useState<Tab>("summary");
  const [menuAt, setMenuAt] = useState<{ x: number; y: number } | null>(null);
  const [scaleOpen, setScaleOpen] = useState(false);
  const [imageOpen, setImageOpen] = useState(false);
  const toast = useStore((s) => s.toast);
  const setAssistantOpen = useStore((s) => s.setAssistantOpen);
  const ask = useAssistant((s) => s.send);

  useEffect(() => {
    if ((tab === "logs" || tab === "terminal") && !isPod) setTab("summary");
  }, [obj, isPod, tab]);

  const run = async (label: string, fn: () => Promise<unknown>, opts: { confirm?: string; danger?: boolean } = {}) => {
    if (opts.confirm) {
      const ok = await confirm({ title: label, message: opts.confirm, confirmLabel: label, danger: opts.danger });
      if (!ok) return;
    }
    try {
      await fn();
      toast("ok", `${label} : fait.`, ref.name);
      onChanged();
    } catch (e) {
      toast("err", (e as Error).message, label);
    }
  };

  const items: MenuItem[] = [];
  if (SCALABLE_KINDS.has(obj.kind)) items.push({ label: "Redimensionner…", onSelect: () => setScaleOpen(true) });
  if (WORKLOAD_KINDS.has(obj.kind) || obj.kind === "Pod") {
    items.push({ label: "Changer l'image…", onSelect: () => setImageOpen(true) });
  }
  if (WORKLOAD_KINDS.has(obj.kind) && obj.kind !== "ReplicaSet") {
    items.push({ label: "Redémarrage progressif", onSelect: () => run("Redémarrage", () => api.resources.restart(cluster, ref)) });
  }
  if (ROLLBACK_KINDS.has(obj.kind)) {
    items.push({
      label: "Retour à la révision précédente",
      onSelect: () => run("Retour arrière", () => api.resources.rollback(cluster, ref), { confirm: `Revenir à la révision précédente de ${ref.name} ?` }),
    });
  }
  if (obj.kind === "Node") {
    const unschedulable = obj.status.toLowerCase().includes("schedulingdisabled") || extraText(obj.extra["unschedulable"]) === "true";
    items.push({
      label: unschedulable ? "Remettre en service (uncordon)" : "Mettre hors service (cordon)",
      onSelect: () => run(unschedulable ? "Remise en service" : "Mise hors service", () => api.resources.cordon(cluster, ref.name, !unschedulable)),
    });
    items.push({
      label: "Vider le nœud (drain)",
      danger: true,
      onSelect: () =>
        run(
          "Vidage du nœud",
          async () => {
            const evicted = await api.resources.drain(cluster, ref.name);
            toast("info", evicted.length ? evicted.join("\n") : "Aucun pod à évincer.", `${evicted.length} pod(s) évincé(s)`);
          },
          { confirm: `Mettre « ${ref.name} » hors service et évincer ses pods éligibles ?`, danger: true },
        ),
    });
  }
  if (items.length) items.push({ separator: true, label: "" });
  items.push({
    label: "Demander à l'assistant",
    icon: <Sparkle size={14} />,
    onSelect: () => {
      setAssistantOpen(true);
      ask(`Explique l'état de ${obj.kind} ${ref.namespace ? ref.namespace + "/" : ""}${ref.name} et signale ce qui cloche, s'il y a lieu.`);
    },
  });
  items.push({ label: "Copier le nom", icon: <Copy size={14} />, onSelect: () => navigator.clipboard.writeText(ref.name) });
  items.push({
    label: "Supprimer",
    icon: <Trash size={14} />,
    danger: true,
    onSelect: () =>
      run("Suppression", () => api.resources.delete(cluster, ref), {
        confirm: `Supprimer définitivement ${obj.kind} ${ref.namespace ? ref.namespace + "/" : ""}${ref.name} ?`,
        danger: true,
      }).then(onClose),
  });

  return (
    <div className="panel fill">
      <div className="toolbar" style={{ gap: 6 }}>
        <Badge tone="info">{obj.kind}</Badge>
        <span className="row gap-4 grow truncate" style={{ minWidth: 0 }}>
          {ref.namespace && <span className="muted">{ref.namespace} /</span>}
          <strong className="truncate selectable" title={ref.name}>
            {ref.name}
          </strong>
        </span>
        <StatusBadge status={obj.status} />
        <ReadyBadge ready={obj.ready} />
        <button className="btn btn-sm" onClick={(e) => setMenuAt({ x: e.clientX, y: e.clientY })}>
          Actions <CaretDown size={12} />
        </button>
        <button className="btn btn-ghost btn-sm btn-icon" onClick={onClose} title="Fermer">
          <X size={14} />
        </button>
      </div>
      <div className="tabs">
        {(
          [
            ["summary", "Résumé"],
            ["yaml", "YAML"],
            ["events", "Évènements"],
            ...(isPod ? ([["logs", "Journaux"], ["terminal", "Terminal"]] as [Tab, string][]) : []),
          ] as [Tab, string][]
        ).map(([id, label]) => (
          <button key={id} className={`tab ${tab === id ? "active" : ""}`} onClick={() => setTab(id)}>
            {label}
          </button>
        ))}
      </div>
      <div className="fill" style={{ overflow: "hidden" }}>
        {tab === "summary" && <SummaryTab obj={obj} />}
        {tab === "yaml" && <YamlTab cluster={cluster} reference={ref} onChanged={onChanged} />}
        {tab === "events" && <EventsTab cluster={cluster} reference={ref} />}
        {tab === "logs" && isPod && <LogsPane cluster={cluster} pod={ref} />}
        {tab === "terminal" && isPod && <TerminalPane cluster={cluster} pod={ref} container={null} />}
      </div>
      <ContextMenu at={menuAt} items={items} onClose={() => setMenuAt(null)} />
      <ScaleDialog open={scaleOpen} onClose={() => setScaleOpen(false)} obj={obj} onSubmit={(n) => run(`Passage à ${n} réplique(s)`, () => api.resources.scale(cluster, ref, n))} />
      <ImageDialog open={imageOpen} onClose={() => setImageOpen(false)} obj={obj} onSubmit={(c, img) => run("Changement d'image", () => api.resources.setImage(cluster, ref, c, img))} />
    </div>
  );
}

function SummaryTab({ obj }: { obj: ObjectSummary }) {
  const goTo = useStore((s) => s.goToResource);
  const [showAnn, setShowAnn] = useState(false);
  const extras = Object.entries(obj.extra);
  const row = (k: string, v: React.ReactNode) => (
    <div className="kv-row" key={k}>
      <div className="kv-key">{k}</div>
      <div className="kv-val selectable">{v}</div>
    </div>
  );
  return (
    <div className="scroll p-16 col gap-12">
      <div className="kv">
        {row("Nom", obj.name)}
        {obj.namespace && row("Namespace", obj.namespace)}
        {row("Type", `${obj.kind} (${obj.apiVersion})`)}
        {obj.uid && row("UID", <span className="mono xs">{obj.uid}</span>)}
        {row("Créé", `${dateTime(obj.createdAt)} (${age(obj.ageSeconds)})`)}
        {row("Statut", <StatusBadge status={obj.status} />)}
        {obj.ready && row("Prêt", <ReadyBadge ready={obj.ready} />)}
        {obj.restarts != null && row("Redémarrages", obj.restarts)}
        {obj.node && row("Nœud", <a href="#" onClick={(e) => { e.preventDefault(); goTo({ kind: "nodes", name: obj.node!, namespace: null }); }}>{obj.node}</a>)}
        {obj.images.length > 0 &&
          row(
            "Images",
            <div className="col gap-4">
              {obj.images.map((i) => (
                <span key={i} className="mono xs" title={i}>
                  {i}
                </span>
              ))}
            </div>,
          )}
        {obj.owners.length > 0 &&
          row(
            "Propriétaires",
            <div className="row wrap">
              {obj.owners.map((o) => (
                <a key={o.kind + o.name} href="#" onClick={(e) => { e.preventDefault(); goTo({ kind: o.kind.toLowerCase() + "s", name: o.name, namespace: obj.namespace }); }}>
                  {o.kind}/{o.name}
                  {o.controller ? " (contrôleur)" : ""}
                </a>
              ))}
            </div>,
          )}
        {extras.map(([k, v]) => row(k, <span className="mono xs" style={{ whiteSpace: "pre-wrap" }}>{extraText(v)}</span>))}
      </div>
      {Object.keys(obj.labels).length > 0 && (
        <div className="col gap-4">
          <div className="xs muted" style={{ fontWeight: 600, textTransform: "uppercase", letterSpacing: "0.04em" }}>Labels</div>
          <div className="row wrap gap-4">
            {Object.entries(obj.labels).map(([k, v]) => (
              <span key={k} className="tag truncate" title={`${k}=${v}`}>
                {k}={v}
              </span>
            ))}
          </div>
        </div>
      )}
      {Object.keys(obj.annotations).length > 0 && (
        <div className="col gap-4">
          <button className="btn btn-ghost btn-sm" style={{ alignSelf: "flex-start" }} onClick={() => setShowAnn(!showAnn)}>
            {showAnn ? "Masquer" : "Afficher"} les annotations ({Object.keys(obj.annotations).length})
          </button>
          {showAnn && (
            <div className="kv">
              {Object.entries(obj.annotations).map(([k, v]) => (
                <div className="kv-row" key={k}>
                  <div className="kv-key mono xs">{k}</div>
                  <div className="kv-val mono xs selectable" style={{ whiteSpace: "pre-wrap", wordBreak: "break-all" }}>{v}</div>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function YamlTab({ cluster, reference, onChanged }: { cluster: string; reference: ResourceRef; onChanged: () => void }) {
  const qc = useQueryClient();
  const toast = useStore((s) => s.toast);
  const setSelectionYaml = useStore((s) => s.setSelectionYaml);
  const key = ["yaml", cluster, reference.plural, reference.namespace, reference.name];
  const q = useQuery({ queryKey: key, queryFn: () => api.resources.yaml(cluster, reference) });
  const [draft, setDraft] = useState<string>("");
  const [dirty, setDirty] = useState(false);

  useEffect(() => {
    if (q.data != null && !dirty) {
      setDraft(q.data);
      setSelectionYaml(q.data);
    }
  }, [q.data, dirty, setSelectionYaml]);

  const replace = useMutation({
    mutationFn: () => api.resources.replaceYaml(cluster, reference, draft),
    onSuccess: () => {
      toast("ok", "Objet remplacé.", reference.name);
      setDirty(false);
      qc.invalidateQueries({ queryKey: key });
      onChanged();
    },
    onError: (e: Error) => toast("err", e.message, "Remplacement"),
  });
  const apply = useMutation({
    mutationFn: () => api.resources.apply(cluster, draft, { namespace: reference.namespace }),
    onSuccess: (out) => {
      const failed = out.items.filter((i) => i.action === "failed");
      if (failed.length) toast("err", failed.map((f) => f.message ?? "").join("\n"), "Application");
      else toast("ok", out.items.map((i) => `${i.resource.name} : ${i.action}`).join("\n"), "Application côté serveur");
      setDirty(false);
      qc.invalidateQueries({ queryKey: key });
      onChanged();
    },
    onError: (e: Error) => toast("err", e.message, "Application"),
  });

  return (
    <div className="fill">
      <div className="toolbar">
        {q.isFetching && <Spinner />}
        {dirty && <Badge tone="warn">modifié</Badge>}
        <span className="grow" />
        <button className="btn btn-sm" onClick={() => { setDirty(false); q.refetch(); }} title="Recharger depuis le cluster">
          <ArrowsClockwise size={14} /> Recharger
        </button>
        <button className="btn btn-sm" onClick={() => navigator.clipboard.writeText(draft).then(() => toast("ok", "YAML copié."))}>
          <Copy size={14} /> Copier
        </button>
        <button className="btn btn-sm" disabled={!dirty || apply.isPending} onClick={() => apply.mutate()} title="Application côté serveur (kubectl apply --server-side)">
          Appliquer
        </button>
        <button
          className="btn btn-primary btn-sm"
          disabled={!dirty || replace.isPending}
          onClick={async () => {
            if (await confirm({ title: "Remplacer l'objet ?", message: `Le manifeste complet de ${reference.name} sera remplacé par le contenu de l'éditeur (kubectl replace).`, confirmLabel: "Remplacer" })) replace.mutate();
          }}
        >
          <FloppyDisk size={14} /> Remplacer
        </button>
      </div>
      {q.error && <Alert tone="err">{(q.error as Error).message}</Alert>}
      <div className="fill">
        <YamlEditor
          value={draft}
          onChange={(v) => {
            setDraft(v);
            setDirty(v !== q.data);
          }}
        />
      </div>
    </div>
  );
}

function EventsTab({ cluster, reference }: { cluster: string; reference: ResourceRef }) {
  const q = useQuery({
    queryKey: ["events", cluster, reference.namespace, reference.kind, reference.name],
    queryFn: () => api.resources.events(cluster, reference.namespace, reference),
    refetchInterval: 15_000,
  });
  if (q.isLoading) return <div className="p-16"><Spinner label="Lecture des évènements…" /></div>;
  if (q.error) return <div className="p-16"><Alert tone="err">{(q.error as Error).message}</Alert></div>;
  if (!q.data?.length) return <div className="empty small">Aucun évènement récent pour cet objet.</div>;
  return (
    <div className="table-wrap">
      <table className="table">
        <thead>
          <tr>
            <th>Quand</th>
            <th>Type</th>
            <th>Raison</th>
            <th>Message</th>
            <th>×</th>
          </tr>
        </thead>
        <tbody>
          {q.data.map((e, i) => (
            <tr key={i} style={{ cursor: "default" }}>
              <td className="muted" title={dateTime(e.lastSeen)}>{relativeTime(e.lastSeen)}</td>
              <td><Badge tone={e.type === "Warning" ? "warn" : "neutral"}>{e.type}</Badge></td>
              <td>{e.reason}</td>
              <td className="selectable" style={{ whiteSpace: "normal" }}>{e.message}</td>
              <td className="num">{e.count}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function ScaleDialog({ open, onClose, obj, onSubmit }: { open: boolean; onClose: () => void; obj: ObjectSummary; onSubmit: (n: number) => Promise<void> }) {
  const current = Number(/\/(\d+)$/.exec(obj.ready ?? "")?.[1] ?? extraText(obj.extra["replicas"]) ?? 1) || 1;
  const [n, setN] = useState(current);
  useEffect(() => setN(current), [current, open]);
  return (
    <Dialog
      open={open}
      title={`Redimensionner ${obj.name}`}
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={onClose}>Annuler</button>
          <button className="btn btn-primary" onClick={() => onSubmit(n).then(onClose)}>Appliquer</button>
        </>
      }
    >
      <Field label="Nombre de répliques" hint={`Actuellement : ${obj.ready ?? "?"}`}>
        <input className="input" type="number" min={0} max={1000} value={n} onChange={(e) => setN(Math.max(0, Number(e.target.value) || 0))} autoFocus style={{ width: 120 }} />
      </Field>
      {n === 0 && <Alert tone="warn">Zéro réplique : la charge de travail sera arrêtée.</Alert>}
    </Dialog>
  );
}

function ImageDialog({ open, onClose, obj, onSubmit }: { open: boolean; onClose: () => void; obj: ObjectSummary; onSubmit: (container: string | null, image: string) => Promise<void> }) {
  const [container, setContainer] = useState("");
  const [image, setImage] = useState(obj.images[0] ?? "");
  useEffect(() => { setImage(obj.images[0] ?? ""); setContainer(""); }, [obj, open]);
  return (
    <Dialog
      open={open}
      title={`Changer l'image de ${obj.name}`}
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={onClose}>Annuler</button>
          <button className="btn btn-primary" disabled={!image.trim()} onClick={() => onSubmit(container.trim() || null, image.trim()).then(onClose)}>Appliquer</button>
        </>
      }
    >
      <Field label="Conteneur" hint="Vide : le conteneur unique.">
        <input className="input" value={container} onChange={(e) => setContainer(e.target.value)} placeholder="app" />
      </Field>
      <Field label="Nouvelle image">
        <input className="input mono" value={image} onChange={(e) => setImage(e.target.value)} autoFocus />
        {obj.images.length > 0 && <div className="hint">Actuellement : {obj.images.map(shortImage).join(", ")}</div>}
      </Field>
    </Dialog>
  );
}

// Ajout de styles clés/valeurs, partagés par le panneau.
const style = document.createElement("style");
style.textContent = `
.kv { display: grid; grid-template-columns: 140px minmax(0, 1fr); gap: 4px 12px; font-size: var(--fs-s); }
.kv-row { display: contents; }
.kv-key { color: var(--fg-muted); font-weight: 600; font-size: var(--fs-xs); text-transform: uppercase; letter-spacing: 0.04em; padding-top: 3px; }
.kv-val { min-width: 0; word-break: break-word; }
`;
document.head.appendChild(style);
