import { ArrowsClockwise, FolderOpen, Globe, HardDrives, Plus, Trash } from "@phosphor-icons/react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { api } from "@/api/client";
import type { ClusterInfo, ContextInfo } from "@/api/types";
import { useClusters } from "@/app/queries";
import { useStore } from "@/app/store";
import { Alert, Badge, Dot, EmptyState, Field, Spinner } from "@/components/Basics";
import { confirm } from "@/components/Confirm";
import { Dialog } from "@/components/Dialog";

export function ClustersView() {
  const clusters = useClusters();
  const cluster = useStore((s) => s.cluster);
  const setCluster = useStore((s) => s.setCluster);
  const toast = useStore((s) => s.toast);
  const filter = useStore((s) => s.filter);
  const qc = useQueryClient();
  const [kubeconfigOpen, setKubeconfigOpen] = useState(false);
  const [remoteOpen, setRemoteOpen] = useState(false);

  const refreshAll = () => qc.invalidateQueries({ queryKey: ["clusters"] });

  const use = async (c: ClusterInfo) => {
    try {
      await api.clusters.select(c.name);
      setCluster(c.name);
      toast("ok", `Cluster courant : « ${c.name} »`);
    } catch (e) {
      toast("err", (e as Error).message);
    }
  };

  const remove = async (c: ClusterInfo) => {
    const ok = await confirm({
      title: `Retirer « ${c.name} » ?`,
      message:
        "Le cluster est retiré de KubeWatch et du fichier d'état. Le cluster lui-même n'est pas touché ; vous pourrez le réimporter.",
      confirmLabel: "Retirer",
      danger: true,
    });
    if (!ok) return;
    try {
      await api.clusters.remove(c.name);
      if (cluster === c.name) setCluster(null);
      toast("ok", `« ${c.name} » retiré.`);
      refreshAll();
    } catch (e) {
      toast("err", (e as Error).message);
    }
  };

  const rows = (clusters.data ?? []).filter((c) =>
    filter ? `${c.name} ${c.server} ${c.context ?? ""}`.toLowerCase().includes(filter.toLowerCase()) : true,
  );

  return (
    <div className="page">
      <div className="page-header">
        <h1>Clusters</h1>
        <span className="grow" />
        {clusters.isFetching && <Spinner />}
        <button className="btn btn-sm" onClick={refreshAll}>
          <ArrowsClockwise size={14} /> Actualiser
        </button>
        <button className="btn btn-sm" onClick={() => setRemoteOpen(true)}>
          <Globe size={14} /> Serveur distant
        </button>
        <button className="btn btn-primary btn-sm" onClick={() => setKubeconfigOpen(true)}>
          <Plus size={14} /> Importer un kubeconfig
        </button>
      </div>

      {clusters.error && <Alert tone="err">{(clusters.error as Error).message}</Alert>}

      {rows.length === 0 ? (
        <EmptyState
          icon={HardDrives}
          title="Aucun cluster enregistré"
          hint="Importez votre kubeconfig : un cluster par contexte, ou seulement le contexte courant."
          action={
            <button className="btn btn-primary" onClick={() => setKubeconfigOpen(true)}>
              <FolderOpen size={14} /> Importer un kubeconfig
            </button>
          }
        />
      ) : (
        <div className="panel">
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th></th>
                  <th>Nom</th>
                  <th>Serveur</th>
                  <th>Version</th>
                  <th>Nœuds</th>
                  <th>Namespaces</th>
                  <th>NS par défaut</th>
                  <th>Métriques</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {rows.map((c) => (
                  <tr key={c.name} className={c.name === cluster ? "selected" : undefined} onClick={() => use(c)}>
                    <td>
                      <Dot tone={c.connected ? "ok" : "err"} />
                    </td>
                    <td>
                      <strong>{c.name}</strong>
                      {c.context && <span className="muted small"> · {c.context}</span>}
                      {c.lastError && (
                        <div className="small" style={{ color: "var(--err)", whiteSpace: "normal", maxWidth: 420 }}>
                          {c.lastError}
                        </div>
                      )}
                    </td>
                    <td className="mono muted">{c.server}</td>
                    <td>{c.version ?? "–"}</td>
                    <td className="num">{c.nodeCount ?? "–"}</td>
                    <td className="num">{c.namespaceCount ?? "–"}</td>
                    <td className="mono">{c.defaultNamespace}</td>
                    <td>{c.metricsAvailable ? <Badge tone="ok">oui</Badge> : <Badge>non</Badge>}</td>
                    <td onClick={(e) => e.stopPropagation()}>
                      <div className="row gap-4">
                        <button className="btn btn-sm" onClick={() => use(c)} disabled={c.name === cluster}>
                          Utiliser
                        </button>
                        <button
                          className="btn btn-ghost btn-sm btn-icon"
                          title="Rafraîchir le catalogue des types"
                          onClick={() =>
                            api.clusters
                              .refreshCatalog(c.name)
                              .then(() => {
                                toast("ok", "Catalogue rafraîchi.");
                                qc.invalidateQueries({ queryKey: ["kinds", c.name] });
                              })
                              .catch((e: Error) => toast("err", e.message))
                          }
                        >
                          <ArrowsClockwise size={14} />
                        </button>
                        <button className="btn btn-ghost btn-sm btn-icon" title="Retirer" onClick={() => remove(c)}>
                          <Trash size={14} color="var(--err)" />
                        </button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      <KubeconfigDialog open={kubeconfigOpen} onClose={() => setKubeconfigOpen(false)} />
      <RemoteDialog open={remoteOpen} onClose={() => setRemoteOpen(false)} />
    </div>
  );
}

function KubeconfigDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const qc = useQueryClient();
  const toast = useStore((s) => s.toast);
  const setCluster = useStore((s) => s.setCluster);
  const [path, setPath] = useState("");
  const [contexts, setContexts] = useState<ContextInfo[] | null>(null);
  const [checked, setChecked] = useState<Record<string, boolean>>({});
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setContexts(null);
    setError(null);
    api.clusters.defaultKubeconfigPath().then((p) => setPath((cur) => cur || p || "")).catch(() => {});
  }, [open]);

  const read = useMutation({
    mutationFn: () => api.clusters.listContexts(path || null),
    onSuccess: (list) => {
      setContexts(list);
      setChecked(Object.fromEntries(list.map((c) => [c.name, c.current || list.length === 1])));
      setError(null);
    },
    onError: (e: Error) => setError(e.message),
  });

  const connect = useMutation({
    mutationFn: async () => {
      const selected = (contexts ?? []).filter((c) => checked[c.name]);
      const results: { name: string; error?: string }[] = [];
      for (const c of selected) {
        try {
          await api.clusters.connect(c.name, { type: "kubeconfig", path: path || null, context: c.name }, true);
          results.push({ name: c.name });
        } catch (e) {
          results.push({ name: c.name, error: (e as Error).message });
        }
      }
      return results;
    },
    onSuccess: (results) => {
      const ok = results.filter((r) => !r.error);
      const ko = results.filter((r) => r.error);
      if (ok.length) {
        toast("ok", ok.length === 1 ? `Cluster « ${ok[0]!.name} » connecté.` : `${ok.length} clusters connectés.`);
        setCluster(ok[0]!.name);
        api.clusters.select(ok[0]!.name).catch(() => {});
      }
      for (const r of ko) toast("err", r.error!, r.name);
      qc.invalidateQueries({ queryKey: ["clusters"] });
      if (ko.length === 0) onClose();
    },
  });

  const selectedCount = Object.values(checked).filter(Boolean).length;

  return (
    <Dialog
      open={open}
      title="Importer un kubeconfig"
      onClose={onClose}
      icon={<FolderOpen size={20} />}
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Annuler
          </button>
          <button
            className="btn btn-primary"
            disabled={!contexts || selectedCount === 0 || connect.isPending}
            onClick={() => connect.mutate()}
          >
            {connect.isPending ? <Spinner /> : null}
            Connecter {selectedCount > 0 ? `(${selectedCount})` : ""}
          </button>
        </>
      }
    >
      <Field label="Fichier kubeconfig" hint="Vide : $KUBECONFIG, puis ~/.kube/config.">
        <div className="row">
          <input className="input grow mono" value={path} onChange={(e) => setPath(e.target.value)} placeholder="~/.kube/config" />
          <button
            className="btn"
            onClick={() => api.clusters.pickFile("Choisir un kubeconfig").then((p) => p && setPath(p)).catch(() => {})}
          >
            Parcourir…
          </button>
          <button className="btn" onClick={() => read.mutate()} disabled={read.isPending}>
            {read.isPending ? <Spinner /> : "Lire les contextes"}
          </button>
        </div>
      </Field>
      {error && <Alert tone="err">{error}</Alert>}
      {contexts && contexts.length === 0 && <Alert tone="warn">Ce fichier ne déclare aucun contexte.</Alert>}
      {contexts && contexts.length > 0 && (
        <div className="col gap-4">
          <div className="row between">
            <span className="small muted">
              {contexts.length} contexte{contexts.length > 1 ? "s" : ""} · chaque contexte devient un cluster KubeWatch
            </span>
            <span className="row gap-4">
              <button className="btn btn-ghost btn-sm" onClick={() => setChecked(Object.fromEntries(contexts.map((c) => [c.name, true])))}>
                Tout
              </button>
              <button className="btn btn-ghost btn-sm" onClick={() => setChecked({})}>
                Aucun
              </button>
            </span>
          </div>
          <div className="panel" style={{ maxHeight: 280, overflow: "auto" }}>
            {contexts.map((c) => (
              <label key={c.name} className="checkbox" style={{ padding: "6px 10px", borderBottom: "1px solid var(--border)" }}>
                <input type="checkbox" checked={!!checked[c.name]} onChange={(e) => setChecked({ ...checked, [c.name]: e.target.checked })} />
                <span className="grow col" style={{ gap: 0 }}>
                  <span>
                    <strong>{c.name}</strong> {c.current && <Badge tone="info">courant</Badge>}
                  </span>
                  <span className="xs muted mono">
                    {c.cluster}
                    {c.server ? ` · ${c.server}` : ""}
                    {c.namespace ? ` · ns ${c.namespace}` : ""}
                  </span>
                </span>
              </label>
            ))}
          </div>
        </div>
      )}
    </Dialog>
  );
}

function RemoteDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const qc = useQueryClient();
  const toast = useStore((s) => s.toast);
  const setCluster = useStore((s) => s.setCluster);
  const [form, setForm] = useState({
    name: "",
    server: "",
    token: "",
    caCertPem: "",
    clientCertPem: "",
    clientKeyPem: "",
    insecure: false,
    namespace: "",
    proxyUrl: "",
  });
  const [advanced, setAdvanced] = useState(false);
  const set = (k: keyof typeof form, v: string | boolean) => setForm({ ...form, [k]: v });

  const connect = useMutation({
    mutationFn: () =>
      api.clusters.connect(
        form.name,
        {
          type: "remote",
          server: form.server.trim(),
          token: form.token.trim() || null,
          caCertPem: form.caCertPem.trim() || null,
          clientCertPem: form.clientCertPem.trim() || null,
          clientKeyPem: form.clientKeyPem.trim() || null,
          insecureSkipTlsVerify: form.insecure,
          namespace: form.namespace.trim() || null,
          proxyUrl: form.proxyUrl.trim() || null,
        },
        true,
      ),
    onSuccess: (info) => {
      toast("ok", `Cluster « ${info.name} » connecté.`);
      setCluster(info.name);
      api.clusters.select(info.name).catch(() => {});
      qc.invalidateQueries({ queryKey: ["clusters"] });
      onClose();
    },
    onError: (e: Error) => toast("err", e.message),
  });

  return (
    <Dialog
      open={open}
      title="Connexion à un serveur d'API"
      onClose={onClose}
      icon={<Globe size={20} />}
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Annuler
          </button>
          <button className="btn btn-primary" disabled={!form.name.trim() || !form.server.trim() || connect.isPending} onClick={() => connect.mutate()}>
            {connect.isPending ? <Spinner /> : null} Connecter
          </button>
        </>
      }
    >
      <div className="row gap-12" style={{ alignItems: "flex-start" }}>
        <Field label="Nom dans KubeWatch">
          <input className="input" value={form.name} onChange={(e) => set("name", e.target.value)} placeholder="prod-eu" />
        </Field>
        <div className="grow">
          <Field label="Serveur d'API">
            <input className="input mono" value={form.server} onChange={(e) => set("server", e.target.value)} placeholder="https://10.0.0.1:6443" />
          </Field>
        </div>
      </div>
      <Field label="Jeton porteur" hint="ServiceAccount ou jeton OIDC déjà échangé. Conservé dans le fichier d'état (0600).">
        <input className="input mono" type="password" value={form.token} onChange={(e) => set("token", e.target.value)} />
      </Field>
      <Field label="Autorité de certification (PEM)">
        <textarea className="input mono" rows={4} value={form.caCertPem} onChange={(e) => set("caCertPem", e.target.value)} placeholder="-----BEGIN CERTIFICATE-----" />
      </Field>
      <label className="checkbox">
        <input type="checkbox" checked={form.insecure} onChange={(e) => set("insecure", e.target.checked)} />
        Ignorer la vérification TLS du serveur (déconseillé)
      </label>
      {form.insecure && <Alert tone="warn">Sans vérification TLS, un attaquant sur le réseau peut usurper le cluster et lire le jeton.</Alert>}
      <button className="btn btn-ghost btn-sm" onClick={() => setAdvanced(!advanced)} style={{ alignSelf: "flex-start" }}>
        {advanced ? "Masquer" : "Afficher"} les options avancées
      </button>
      {advanced && (
        <>
          <Field label="Certificat client (PEM)">
            <textarea className="input mono" rows={3} value={form.clientCertPem} onChange={(e) => set("clientCertPem", e.target.value)} />
          </Field>
          <Field label="Clé privée client (PEM)">
            <textarea className="input mono" rows={3} value={form.clientKeyPem} onChange={(e) => set("clientKeyPem", e.target.value)} />
          </Field>
          <div className="row gap-12">
            <Field label="Namespace par défaut">
              <input className="input" value={form.namespace} onChange={(e) => set("namespace", e.target.value)} placeholder="default" />
            </Field>
            <div className="grow">
              <Field label="Proxy HTTP / SOCKS5">
                <input className="input mono" value={form.proxyUrl} onChange={(e) => set("proxyUrl", e.target.value)} placeholder="socks5://127.0.0.1:1080" />
              </Field>
            </div>
          </div>
        </>
      )}
    </Dialog>
  );
}
