// Dialogue de création / modification d'un surveillant de mise à jour.
// Formulaire d'une surveillance : mêmes champs que `WatcherSpec` côté Rust,
// mêmes règles de validation, mêmes messages.
import { Eye } from "@phosphor-icons/react";
import { useMutation } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { api } from "@/api/client";
import type { UpdateChannel, UpdateSource, WatcherSpec, WatchersBundle } from "@/api/updates";
import { useClusters, useNamespaces } from "@/app/queries";
import { useStore } from "@/app/store";
import { Alert, Field, Spinner } from "./Basics";
import { Dialog } from "./Dialog";
import { CHANNELS, humanInterval } from "./UpdatesCommon";
import "@/views/Updates.css";

type SourceMode = "github" | "image" | "chart";

/** Kinds proposés ; un kind existant hors liste reste sélectionnable. */
const KINDS = ["Deployment", "StatefulSet", "DaemonSet", "CronJob"];

const MAINTENANCE_RE = /^([01]\d|2[0-3]):[0-5]\d-([01]\d|2[0-3]):[0-5]\d$/;

interface Form {
  name: string;
  enabled: boolean;
  cluster: string;
  kind: string;
  namespace: string;
  targetName: string;
  container: string;
  sourceMode: SourceMode;
  ghOwner: string;
  ghRepo: string;
  ghTagPrefix: string;
  image: string;
  chartRepo: string;
  chartName: string;
  channel: UpdateChannel;
  allowPrerelease: boolean;
  constraint: string;
  ignoreText: string;
  autoApply: boolean;
  checkIntervalSeconds: number;
  maintenanceWindow: string;
}

/** Formulaire prérempli à partir d'un surveillant (existant ou brouillon). */
function formFrom(w: WatcherSpec): Form {
  const f: Form = {
    name: w.name,
    enabled: w.enabled,
    cluster: w.cluster,
    kind: w.target.kind || "Deployment",
    namespace: w.target.namespace ?? "",
    targetName: w.target.name,
    container: w.container ?? "",
    sourceMode: "image",
    ghOwner: "",
    ghRepo: "",
    ghTagPrefix: "",
    image: "",
    chartRepo: "",
    chartName: "",
    channel: w.policy.channel,
    allowPrerelease: w.policy.allowPrerelease,
    constraint: w.policy.constraint ?? "",
    ignoreText: w.policy.ignore.join("\n"),
    autoApply: w.policy.autoApply,
    checkIntervalSeconds: w.policy.checkIntervalSeconds,
    maintenanceWindow: w.policy.maintenanceWindow ?? "",
  };
  switch (w.source.type) {
    case "githubRelease":
      f.sourceMode = "github";
      f.ghOwner = w.source.owner;
      f.ghRepo = w.source.repo;
      f.ghTagPrefix = w.source.tagPrefix ?? "";
      break;
    case "containerRegistry":
      f.sourceMode = "image";
      f.image = w.source.image;
      break;
    case "helmChart":
      f.sourceMode = "chart";
      f.chartRepo = w.source.repo;
      f.chartName = w.source.chart;
      break;
  }
  return f;
}

const blank = (s: string): string | null => {
  const t = s.trim();
  return t ? t : null;
};

/** Construit le surveillant à enregistrer, ou explique ce qui manque. */
function buildWatcher(f: Form, base: WatcherSpec): { ok: true; spec: WatcherSpec } | { ok: false; error: string } {
  const name = f.name.trim();
  if (!name) return { ok: false, error: "Donnez un nom au surveillant." };
  const cluster = f.cluster.trim();
  if (!cluster) return { ok: false, error: "Choisissez le cluster qui porte la cible." };
  const kind = f.kind.trim();
  if (!kind) return { ok: false, error: "Choisissez le type de l'objet surveillé." };
  const targetName = f.targetName.trim();
  if (!targetName) return { ok: false, error: "Indiquez le nom de l'objet surveillé." };

  let source: UpdateSource;
  switch (f.sourceMode) {
    case "github": {
      const owner = f.ghOwner.trim();
      const repo = f.ghRepo.trim();
      if (!owner || !repo) return { ok: false, error: "Renseignez le propriétaire et le nom du dépôt GitHub." };
      source = { type: "githubRelease", owner, repo, tagPrefix: blank(f.ghTagPrefix) };
      break;
    }
    case "image": {
      const image = f.image.trim();
      if (!image) return { ok: false, error: "Indiquez la référence d'image à suivre." };
      source = { type: "containerRegistry", image };
      break;
    }
    case "chart": {
      const repo = f.chartRepo.trim();
      const chart = f.chartName.trim();
      if (!repo || !chart) return { ok: false, error: "Renseignez le dépôt et le nom du chart Helm." };
      source = { type: "helmChart", repo, chart };
      break;
    }
  }

  const maintenanceWindow = blank(f.maintenanceWindow);
  if (maintenanceWindow && !MAINTENANCE_RE.test(maintenanceWindow)) {
    return { ok: false, error: "Fenêtre de maintenance attendue au format HH:MM-HH:MM (UTC)." };
  }

  const interval = Number.isFinite(f.checkIntervalSeconds) ? Math.floor(f.checkIntervalSeconds) : 3600;

  return {
    ok: true,
    spec: {
      ...base,
      name,
      enabled: f.enabled,
      cluster,
      target: { namespace: blank(f.namespace), kind, name: targetName },
      container: blank(f.container),
      source,
      policy: {
        channel: f.channel,
        allowPrerelease: f.allowPrerelease,
        constraint: blank(f.constraint),
        ignore: f.ignoreText
          .split("\n")
          .map((l) => l.trim())
          .filter(Boolean),
        autoApply: f.autoApply,
        checkIntervalSeconds: Math.max(60, interval),
        maintenanceWindow,
      },
    },
  };
}

export function UpdatesWatcherDialog({
  open,
  watcher,
  isNew,
  onClose,
  onSaved,
}: {
  open: boolean;
  /** Surveillant existant (modification) ou brouillon complet (création). */
  watcher: WatcherSpec | null;
  isNew: boolean;
  onClose: () => void;
  onSaved: (bundle: WatchersBundle) => void;
}) {
  const toast = useStore((s) => s.toast);
  const clusters = useClusters();
  const [form, setForm] = useState<Form | null>(null);
  const [error, setError] = useState<string | null>(null);
  const namespaces = useNamespaces(form?.cluster || null);

  useEffect(() => {
    if (!open || !watcher) return;
    setForm(formFrom(watcher));
    setError(null);
  }, [open, watcher]);

  const save = useMutation({
    mutationFn: (spec: WatcherSpec) => api.updates.upsertWatcher(spec),
    onSuccess: (bundle, spec) => {
      toast("ok", isNew ? `Surveillant « ${spec.name} » créé.` : `Surveillant « ${spec.name} » enregistré.`);
      onSaved(bundle);
    },
    onError: (e: Error) => setError(e.message),
  });

  if (!form || !watcher) return null;
  const set = <K extends keyof Form>(k: K, v: Form[K]) => setForm({ ...form, [k]: v });

  const submit = () => {
    const built = buildWatcher(form, watcher);
    if (!built.ok) {
      setError(built.error);
      return;
    }
    setError(null);
    save.mutate(built.spec);
  };

  const clusterNames = (clusters.data ?? []).map((c) => c.name);
  if (form.cluster && !clusterNames.includes(form.cluster)) clusterNames.unshift(form.cluster);
  const kinds = KINDS.includes(form.kind) ? KINDS : [form.kind, ...KINDS];

  const modes: { id: SourceMode; label: string }[] = [
    { id: "github", label: "Release GitHub" },
    { id: "image", label: "Registre d'images" },
    { id: "chart", label: "Chart Helm" },
  ];

  return (
    <Dialog
      open={open}
      size="lg"
      title={isNew ? "Nouveau surveillant" : `Modifier « ${watcher.name} »`}
      onClose={onClose}
      icon={<Eye size={20} />}
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Annuler
          </button>
          <button className="btn btn-primary" disabled={save.isPending} onClick={submit}>
            {save.isPending ? <Spinner /> : null} Enregistrer
          </button>
        </>
      }
    >
      <div className="updates-form-title">Cible</div>
      <div className="updates-form-grid">
        <Field label="Nom du surveillant">
          <input className="input" value={form.name} onChange={(e) => set("name", e.target.value)} placeholder="nginx production" autoFocus />
        </Field>
        <Field label="Cluster">
          <select className="select" value={form.cluster} onChange={(e) => set("cluster", e.target.value)}>
            {!form.cluster && <option value="">— choisir —</option>}
            {clusterNames.map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Type d'objet">
          <select className="select" value={form.kind} onChange={(e) => set("kind", e.target.value)}>
            {kinds.map((k) => (
              <option key={k} value={k}>
                {k}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Namespace" hint="Vide : ressource de portée cluster.">
          <input
            className="input"
            list="updates-namespaces"
            value={form.namespace}
            onChange={(e) => set("namespace", e.target.value)}
            placeholder="default"
            autoComplete="off"
          />
          <datalist id="updates-namespaces">
            {(namespaces.data ?? []).map((ns) => (
              <option key={ns} value={ns} />
            ))}
          </datalist>
        </Field>
        <Field label="Nom de l'objet">
          <input className="input mono" value={form.targetName} onChange={(e) => set("targetName", e.target.value)} placeholder="mon-api" />
        </Field>
        <Field label="Conteneur" hint="Vide : premier conteneur du pod.">
          <input className="input mono" value={form.container} onChange={(e) => set("container", e.target.value)} placeholder="premier conteneur" />
        </Field>
      </div>

      <div className="updates-form-title">Source des versions</div>
      <div className="btn-group" style={{ alignSelf: "flex-start" }}>
        {modes.map((m) => (
          <button key={m.id} className={`btn btn-sm ${form.sourceMode === m.id ? "active" : ""}`} onClick={() => set("sourceMode", m.id)}>
            {m.label}
          </button>
        ))}
      </div>
      {form.sourceMode === "github" && (
        <div className="updates-form-grid">
          <Field label="Propriétaire">
            <input className="input mono" value={form.ghOwner} onChange={(e) => set("ghOwner", e.target.value)} placeholder="kubernetes" />
          </Field>
          <Field label="Dépôt">
            <input className="input mono" value={form.ghRepo} onChange={(e) => set("ghRepo", e.target.value)} placeholder="ingress-nginx" />
          </Field>
          <Field label="Préfixe de tag" hint="Retiré des tags avant l'analyse semver (ex. « controller-v »).">
            <input className="input mono" value={form.ghTagPrefix} onChange={(e) => set("ghTagPrefix", e.target.value)} placeholder="controller-v" />
          </Field>
        </div>
      )}
      {form.sourceMode === "image" && (
        <div className="updates-form-grid">
          <Field label="Image" hint="Référence d'image, tag inclus ou non ; les tags du registre sont suivis.">
            <input className="input mono" value={form.image} onChange={(e) => set("image", e.target.value)} placeholder="docker.io/library/nginx:1.27" />
          </Field>
        </div>
      )}
      {form.sourceMode === "chart" && (
        <div className="updates-form-grid">
          <Field label="Dépôt de charts">
            <input className="input mono" value={form.chartRepo} onChange={(e) => set("chartRepo", e.target.value)} placeholder="https://charts.bitnami.com/bitnami" />
          </Field>
          <Field label="Chart">
            <input className="input mono" value={form.chartName} onChange={(e) => set("chartName", e.target.value)} placeholder="postgresql" />
          </Field>
        </div>
      )}

      <div className="updates-form-title">Politique</div>
      <div className="updates-form-grid">
        <Field label="Canal" hint={CHANNELS.find((c) => c.id === form.channel)?.hint}>
          <select className="select" value={form.channel} onChange={(e) => set("channel", e.target.value as UpdateChannel)}>
            {CHANNELS.map((c) => (
              <option key={c.id} value={c.id}>
                {c.label}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Contrainte semver" hint="Facultative, ex. « >=1.2, <2 ».">
          <input className="input mono" value={form.constraint} onChange={(e) => set("constraint", e.target.value)} placeholder=">=1.2, <2" />
        </Field>
        <Field label="Intervalle de vérification (secondes)" hint={`60 s minimum · soit ${humanInterval(Math.max(60, form.checkIntervalSeconds || 0))}.`}>
          <input
            className="input"
            type="number"
            min={60}
            step={60}
            value={form.checkIntervalSeconds}
            onChange={(e) => set("checkIntervalSeconds", Number(e.target.value))}
          />
        </Field>
        <Field label="Fenêtre de maintenance" hint="UTC, format HH:MM-HH:MM ; vide : à tout moment.">
          <input className="input mono" value={form.maintenanceWindow} onChange={(e) => set("maintenanceWindow", e.target.value)} placeholder="01:00-05:00" />
        </Field>
        <div className="span-2">
          <Field label="Versions ignorées" hint="Une version ou un tag par ligne.">
            <textarea className="input mono" rows={3} value={form.ignoreText} onChange={(e) => set("ignoreText", e.target.value)} placeholder={"1.2.3\nlatest"} />
          </Field>
        </div>
      </div>
      <div className="row gap-16 wrap">
        <label className="checkbox">
          <input type="checkbox" checked={form.allowPrerelease} onChange={(e) => set("allowPrerelease", e.target.checked)} /> Accepter les pré-versions
        </label>
        <label className="checkbox">
          <input type="checkbox" checked={form.autoApply} onChange={(e) => set("autoApply", e.target.checked)} /> Appliquer automatiquement les mises à jour détectées
        </label>
        <label className="checkbox">
          <input type="checkbox" checked={form.enabled} onChange={(e) => set("enabled", e.target.checked)} /> Surveillant actif
        </label>
      </div>
      {form.autoApply && (
        <Alert tone="warn">L'application automatique modifie le déploiement sans confirmation, dans la fenêtre de maintenance si elle est définie.</Alert>
      )}
      {error && <Alert tone="err">{error}</Alert>}
    </Dialog>
  );
}
