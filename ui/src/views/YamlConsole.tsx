// Console YAML : simulation, différences, application et suppression de manifestes.
import { TerminalWindow } from "@phosphor-icons/react";
import { useMutation } from "@tanstack/react-query";
import { useState } from "react";
import { api } from "@/api/client";
import type { ApplyOutcome, DiffItem } from "@/api/types";
import { useNamespaces } from "@/app/queries";
import { useNamespace, useStore } from "@/app/store";
import { Alert, Badge, Spinner } from "@/components/Basics";
import { confirm } from "@/components/Confirm";
import { Dialog } from "@/components/Dialog";
import { YamlEditor } from "@/components/YamlEditor";

const ACTION_TONE = { created: "ok", configured: "info", unchanged: "neutral", dryRun: "info", deleted: "warn", failed: "err" } as const;
const ACTION_LABEL = { created: "créé", configured: "configuré", unchanged: "inchangé", dryRun: "simulé", deleted: "supprimé", failed: "échec" } as const;

export function YamlConsole({ cluster, onChanged }: { cluster: string; onChanged: () => void }) {
  const open = useStore((s) => s.yamlConsole.open);
  const draft = useStore((s) => s.yamlConsole.draft);
  const setDraft = useStore((s) => s.setYamlDraft);
  const close = useStore((s) => s.closeYamlConsole);
  const current = useNamespace();
  const namespaces = useNamespaces(cluster);
  const [namespace, setNamespace] = useState<string>(current ?? "default");
  const [force, setForce] = useState(false);
  const [result, setResult] = useState<{ kind: "outcome"; value: ApplyOutcome; title: string } | { kind: "diff"; value: DiffItem[] } | { kind: "error"; value: string } | null>(null);

  const ns = namespace || null;
  const simulate = useMutation({
    mutationFn: () => api.resources.apply(cluster, draft, { dryRun: true, force, namespace: ns }),
    onSuccess: (v) => setResult({ kind: "outcome", value: v, title: "Simulation" }),
    onError: (e: Error) => setResult({ kind: "error", value: e.message }),
  });
  const diff = useMutation({
    mutationFn: () => api.resources.diff(cluster, draft, ns),
    onSuccess: (v) => setResult({ kind: "diff", value: v }),
    onError: (e: Error) => setResult({ kind: "error", value: e.message }),
  });
  const apply = useMutation({
    mutationFn: () => api.resources.apply(cluster, draft, { force, namespace: ns }),
    onSuccess: (v) => {
      setResult({ kind: "outcome", value: v, title: "Application" });
      onChanged();
    },
    onError: (e: Error) => setResult({ kind: "error", value: e.message }),
  });
  const del = useMutation({
    mutationFn: () => api.resources.deleteYaml(cluster, draft, ns),
    onSuccess: (v) => {
      setResult({ kind: "outcome", value: v, title: "Suppression" });
      onChanged();
    },
    onError: (e: Error) => setResult({ kind: "error", value: e.message }),
  });
  const busy = simulate.isPending || diff.isPending || apply.isPending || del.isPending;
  const empty = !draft.trim();

  return (
    <Dialog
      open={open}
      title="Console YAML"
      onClose={close}
      size="lg"
      icon={<TerminalWindow size={20} />}
      footer={
        <>
          <label className="row small" style={{ marginRight: "auto" }}>
            <span className="muted">Namespace par défaut</span>
            <select className="select select-sm" value={namespace} onChange={(e) => setNamespace(e.target.value)}>
              <option value="">(aucun)</option>
              {namespaces.data?.map((n) => (
                <option key={n} value={n}>{n}</option>
              ))}
            </select>
            <label className="checkbox" title="Reprend de force les champs détenus par un autre gestionnaire">
              <input type="checkbox" checked={force} onChange={(e) => setForce(e.target.checked)} /> Forcer
            </label>
          </label>
          {busy && <Spinner />}
          <button className="btn btn-danger" disabled={empty || busy} onClick={async () => (await confirm({ title: "Supprimer les objets du manifeste ?", message: "Chaque document du manifeste sera supprimé du cluster.", confirmLabel: "Supprimer", danger: true })) && del.mutate()}>
            Supprimer
          </button>
          <button className="btn" disabled={empty || busy} onClick={() => diff.mutate()}>Différences</button>
          <button className="btn" disabled={empty || busy} onClick={() => simulate.mutate()}>Simuler</button>
          <button className="btn btn-primary" disabled={empty || busy} onClick={async () => (await confirm({ title: "Appliquer le manifeste ?", message: `Application côté serveur sur « ${cluster} ».`, confirmLabel: "Appliquer" })) && apply.mutate()}>
            Appliquer
          </button>
        </>
      }
    >
      <div style={{ height: "46vh", minHeight: 240, display: "flex", border: "1px solid var(--border)", borderRadius: "var(--radius-s)", overflow: "hidden" }}>
        <YamlEditor value={draft} onChange={setDraft} />
      </div>
      {result?.kind === "error" && <Alert tone="err">{result.value}</Alert>}
      {result?.kind === "outcome" && (
        <div className="panel">
          <div className="toolbar small">
            <strong>{result.title}</strong>
            <span className="muted">
              {result.value.items.length} document{result.value.items.length > 1 ? "s" : ""}, {result.value.failed} en échec
            </span>
          </div>
          <table className="table">
            <tbody>
              {result.value.items.map((it, i) => (
                <tr key={i} style={{ cursor: "default" }}>
                  <td>{it.resource.kind}</td>
                  <td className="mono">{it.resource.namespace ? `${it.resource.namespace}/` : ""}{it.resource.name}</td>
                  <td><Badge tone={ACTION_TONE[it.action]}>{ACTION_LABEL[it.action]}</Badge></td>
                  <td className="selectable" style={{ whiteSpace: "normal" }}>{it.message}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {result?.kind === "diff" && (
        <div className="col gap-8">
          {result.value.length === 0 && <Alert tone="info">Aucun document.</Alert>}
          {result.value.map((d, i) => (
            <div key={i} className="col gap-4">
              <div className="small"><strong>{d.resource.kind}</strong> <span className="mono">{d.resource.namespace ? `${d.resource.namespace}/` : ""}{d.resource.name}</span></div>
              <pre className="code" style={{ maxHeight: 260 }}>{d.diff.trim() || "(identique)"}</pre>
            </div>
          ))}
        </div>
      )}
    </Dialog>
  );
}
