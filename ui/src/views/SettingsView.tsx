import { Check, Pencil, Plus, Sparkle, Trash } from "@phosphor-icons/react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { api } from "@/api/client";
import type { Acceleration, AppliedRender, ClaudeCodeAccount, ModelInfo, ProfileUpdate, ProfileView, ProviderKind, TextRendering } from "@/api/types";
import type { UpdateChannel, UpdaterSettings } from "@/api/updates";
import { useAiSettings } from "@/app/queries";
import { useStore, type Theme } from "@/app/store";
import { Alert, Badge, Field, Section, Spinner } from "@/components/Basics";
import { confirm } from "@/components/Confirm";
import { Dialog } from "@/components/Dialog";

export function SettingsView() {
  return (
    <div className="page" style={{ maxWidth: 980 }}>
      <div className="page-header">
        <h1>Réglages</h1>
      </div>
      <AppearanceSection />
      <AiSection />
      <UpdaterSection />
      <AboutSection />
    </div>
  );
}

/** Libellés des modes d'accélération, du plus accéléré au plus compatible. */
const ACCELERATIONS: { id: Acceleration; label: string }[] = [
  { id: "full", label: "Tout GPU" },
  { id: "auto", label: "Automatique" },
  { id: "cpuPainting", label: "Peinture CPU" },
  { id: "off", label: "Désactivée" },
];

/** Libellés de la rastérisation du texte. */
const TEXT_RENDERINGS: { id: TextRendering; label: string }[] = [
  { id: "system", label: "Système" },
  { id: "sharp", label: "Net" },
  { id: "smooth", label: "Lissé" },
];

const APPLIED_LABEL: Record<AppliedRender, string> = {
  gpu: "tout le rendu sur la carte graphique",
  hybrid: "composition accélérée, peinture GPU puis processeur",
  cpuPainting: "composition accélérée, peinture sur le processeur",
  noDmabuf: "DMA-BUF coupé par l'environnement : plus d'accélération",
  software: "rendu logiciel",
};

function AppearanceSection() {
  const theme = useStore((s) => s.theme);
  const setTheme = useStore((s) => s.setTheme);
  const autoRefresh = useStore((s) => s.autoRefresh);
  const toggleAutoRefresh = useStore((s) => s.toggleAutoRefresh);
  const toast = useStore((s) => s.toast);
  const qc = useQueryClient();
  const render = useQuery({ queryKey: ["render-settings"], queryFn: api.render.get });
  const setAcceleration = useMutation({
    mutationFn: (a: Acceleration) => api.render.setAcceleration(a),
    onSuccess: (v) => qc.setQueryData(["render-settings"], v),
    onError: (e: Error) => toast("err", e.message),
  });
  const setText = useMutation({
    mutationFn: (t: TextRendering) => api.render.setText(t),
    onSuccess: (v) => qc.setQueryData(["render-settings"], v),
    onError: (e: Error) => toast("err", e.message),
  });
  const options: { id: Theme; label: string }[] = [
    { id: "auto", label: "Système" },
    { id: "light", label: "Clair" },
    { id: "dark", label: "Sombre" },
  ];
  const r = render.data;
  return (
    <Section title="Apparence et comportement">
      <div className="row gap-16">
        <Field label="Thème">
          <div className="btn-group">
            {options.map((o) => (
              <button key={o.id} className={`btn btn-sm ${theme === o.id ? "active" : ""}`} onClick={() => setTheme(o.id)}>
                {o.label}
              </button>
            ))}
          </div>
        </Field>
        <Field label="Actualisation automatique" hint="Listes toutes les 5 s, synthèse toutes les 10 s.">
          <label className="checkbox">
            <input type="checkbox" checked={autoRefresh} onChange={toggleAutoRefresh} /> Activée
          </label>
        </Field>
      </div>

      {r?.supported && (
        <Field
          label="Accélération matérielle"
          hint={
            r.forcedByEnv ? (
              <>
                Imposée par l'environnement (WEBKIT_DISABLE_…) : le choix ci-dessus reste sans effet. En vigueur :{" "}
                {APPLIED_LABEL[r.applied]}.
              </>
            ) : (
              <>
                <strong>Tout GPU</strong> : composition et peinture des pages entièrement sur la carte graphique.{" "}
                <strong>Automatique</strong> : peinture au GPU d'abord, le processeur prenant les débordements.{" "}
                <strong>Peinture CPU</strong> : composition accélérée, pages peintes par le processeur — un repli si un
                pilote peint mal, sans effet sur la netteté du texte. <strong>Désactivée</strong> : rendu logiciel, en dernier
                recours si la fenêtre reste noire ou montre des artefacts. En vigueur depuis le lancement :{" "}
                {APPLIED_LABEL[r.applied]}.
              </>
            )
          }
        >
          <div className="btn-group">
            {ACCELERATIONS.map((o) => (
              <button
                key={o.id}
                className={`btn btn-sm ${r.acceleration === o.id ? "active" : ""}`}
                disabled={r.forcedByEnv || setAcceleration.isPending}
                onClick={() => setAcceleration.mutate(o.id)}
              >
                {o.label}
              </button>
            ))}
          </div>
        </Field>
      )}
      {r?.supported && (
        <Field
          label="Rendu du texte"
          hint={
            <>
              <strong>Système</strong> : les réglages du bureau. <strong>Net</strong> : anticrénelage sous-pixel RGB et
              hinting complet — le plus lisible sous 100 ppp, à condition que la dalle range ses sous-pixels dans l'ordre
              RGB, sans quoi les bords se teintent. <strong>Lissé</strong> : niveaux de gris, formes fidèles et bords plus
              doux. S'applique au prochain démarrage : le processus web garde les polices qu'il a déjà construites.
            </>
          }
        >
          <div className="btn-group">
            {TEXT_RENDERINGS.map((o) => (
              <button
                key={o.id}
                className={`btn btn-sm ${r.text === o.id ? "active" : ""}`}
                disabled={setText.isPending}
                onClick={() => setText.mutate(o.id)}
              >
                {o.label}
              </button>
            ))}
          </div>
        </Field>
      )}
      {r?.restartNeeded && (
        <Alert tone="info">
          <div className="row gap-8 grow" style={{ alignItems: "center" }}>
            <span className="grow">Le mode de rendu choisi s'appliquera au prochain démarrage de KubeWatch.</span>
            <button className="btn btn-sm" onClick={() => api.restart().catch((e: Error) => toast("err", e.message))}>
              Redémarrer maintenant
            </button>
          </div>
        </Alert>
      )}
    </Section>
  );
}

// --- Assistant IA -----------------------------------------------------------

const KIND_LABEL: Record<ProviderKind, string> = {
  anthropic: "Anthropic (Claude)",
  openAi: "OpenAI (ChatGPT)",
  openAiCompatible: "Compatible OpenAI",
};

interface Preset {
  id: string;
  label: string;
  kind: ProviderKind;
  baseUrl: string;
  model: string;
  hint: string;
}

const PRESETS: Preset[] = [
  { id: "anthropic", label: "Claude (Anthropic)", kind: "anthropic", baseUrl: "https://api.anthropic.com", model: "claude-opus-5", hint: "Votre compte Claude Code, ou une clé « sk-ant-… » depuis console.anthropic.com." },
  { id: "openai", label: "ChatGPT (OpenAI)", kind: "openAi", baseUrl: "https://api.openai.com/v1", model: "gpt-5", hint: "Clé « sk-… » depuis platform.openai.com." },
  { id: "lmstudio", label: "LM Studio (local)", kind: "openAiCompatible", baseUrl: "http://localhost:1234/v1", model: "", hint: "Démarrez le serveur local dans LM Studio (onglet Developer), puis listez les modèles." },
  { id: "ollama", label: "Ollama (local)", kind: "openAiCompatible", baseUrl: "http://localhost:11434/v1", model: "", hint: "Ollama expose une API compatible OpenAI sur /v1." },
  { id: "custom", label: "Autre serveur compatible OpenAI", kind: "openAiCompatible", baseUrl: "", model: "", hint: "llama.cpp, Jan, vLLM, LiteLLM, proxy d'entreprise…" },
];

/** Libellé du compte détecté : courriel, organisation, formule. */
function accountLabel(a: ClaudeCodeAccount): string {
  const parts = [a.email, a.organization, a.subscription ? `formule ${a.subscription}` : null].filter(Boolean);
  return parts.length > 0 ? parts.join(" — ") : a.dir;
}

function AiSection() {
  const settings = useAiSettings();
  const qc = useQueryClient();
  const toast = useStore((s) => s.toast);
  const [editing, setEditing] = useState<ProfileView | null | "new">(null);
  const claudeCode = useQuery({ queryKey: ["claude-code-account"], queryFn: api.ai.claudeCodeAccount });
  const [general, setGeneral] = useState({ toolsEnabled: true, maxToolRounds: 8, extraInstructions: "" });

  useEffect(() => {
    if (settings.data) {
      setGeneral({
        toolsEnabled: settings.data.toolsEnabled,
        maxToolRounds: settings.data.maxToolRounds,
        extraInstructions: settings.data.extraInstructions,
      });
    }
  }, [settings.data]);

  const invalidate = () => qc.invalidateQueries({ queryKey: ["ai-settings"] });

  const setActive = useMutation({
    mutationFn: (id: string) => api.ai.setActive(id),
    onSuccess: invalidate,
    onError: (e: Error) => toast("err", e.message),
  });
  const adopt = useMutation({
    mutationFn: () => api.ai.useClaudeCode(),
    onSuccess: (p) => {
      toast("ok", `« ${p.name} » est prêt : l'assistant répond avec votre compte Claude.`);
      invalidate();
    },
    onError: (e: Error) => toast("err", e.message),
  });
  const remove = async (p: ProfileView) => {
    const ok = await confirm({
      title: `Supprimer « ${p.name} » ?`,
      message: p.useClaudeCode
        ? "Le profil disparaît des réglages ; le dossier ~/.claude n'est pas touché."
        : "La clé d'API enregistrée est effacée.",
      confirmLabel: "Supprimer",
      danger: true,
    });
    if (!ok) return;
    api.ai
      .removeProfile(p.id)
      .then(() => {
        toast("ok", "Profil supprimé.");
        invalidate();
      })
      .catch((e: Error) => toast("err", e.message));
  };
  const saveGeneral = useMutation({
    mutationFn: () => api.ai.setGeneral(general.toolsEnabled, general.maxToolRounds, general.extraInstructions),
    onSuccess: () => {
      toast("ok", "Réglages de l'assistant enregistrés.");
      invalidate();
    },
    onError: (e: Error) => toast("err", e.message),
  });

  const dirty =
    !!settings.data &&
    (settings.data.toolsEnabled !== general.toolsEnabled ||
      settings.data.maxToolRounds !== general.maxToolRounds ||
      settings.data.extraInstructions !== general.extraInstructions);

  return (
    <Section
      title="Assistant IA"
      actions={
        <button className="btn btn-primary btn-sm" onClick={() => setEditing("new")}>
          <Plus size={14} /> Ajouter un fournisseur
        </button>
      }
    >
      <p className="muted small" style={{ margin: 0 }}>
        Claude, ChatGPT ou un modèle local (LM Studio, Ollama…). Les clés sont conservées dans le dossier d'état, en lecture
        seule pour votre compte, et ne quittent la machine que vers le fournisseur choisi.
      </p>
      {claudeCode.data && !settings.data?.profiles.some((p) => p.useClaudeCode) && (
        <Alert tone={claudeCode.data.expired ? "warn" : "info"}>
          <div className="row gap-8 grow" style={{ alignItems: "center" }}>
            <Sparkle size={16} />
            <span className="grow">
              {claudeCode.data.expired ? (
                <>
                  Claude Code est installé ({accountLabel(claudeCode.data)}) mais son jeton a expiré : lancez « claude » dans un
                  terminal pour le renouveler.
                </>
              ) : (
                <>
                  Claude Code est connecté sur cette machine ({accountLabel(claudeCode.data)}) : l'assistant peut répondre avec ce
                  compte, sans clé d'API.
                </>
              )}
            </span>
            <button className="btn btn-sm" disabled={claudeCode.data.expired || adopt.isPending} onClick={() => adopt.mutate()}>
              {adopt.isPending ? <Spinner /> : null} Utiliser ce compte
            </button>
          </div>
        </Alert>
      )}
      {settings.data && settings.data.profiles.length === 0 && !claudeCode.data && (
        <Alert tone="info">
          <Sparkle size={16} /> Aucun fournisseur configuré : ajoutez-en un pour activer l'assistant (Ctrl+J).
        </Alert>
      )}
      {settings.data && settings.data.profiles.length > 0 && (
        <div className="panel">
          <table className="table">
            <thead>
              <tr>
                <th>Actif</th>
                <th>Nom</th>
                <th>Fournisseur</th>
                <th>Modèle</th>
                <th>Adresse</th>
                <th>Clé</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {settings.data.profiles.map((p) => {
                const active = settings.data?.activeProfile === p.id;
                return (
                  <tr key={p.id} className={active ? "selected" : undefined} onClick={() => !active && setActive.mutate(p.id)}>
                    <td>{active ? <Check size={16} color="var(--accent)" weight="bold" /> : <span className="faint">–</span>}</td>
                    <td>
                      <strong>{p.name}</strong>
                    </td>
                    <td>{KIND_LABEL[p.kind]}</td>
                    <td className="mono">{p.model || <span className="faint">non choisi</span>}</td>
                    <td className="mono muted small">{p.baseUrl}</td>
                    <td>
                      {p.useClaudeCode ? (
                        <Badge tone="ok" title={claudeCode.data ? `Lu dans ${claudeCode.data.dir}` : undefined}>
                          compte Claude Code
                        </Badge>
                      ) : p.apiKeySet ? (
                        <Badge tone="ok">enregistrée</Badge>
                      ) : p.kind === "openAiCompatible" ? (
                        <Badge>facultative</Badge>
                      ) : (
                        <Badge tone="warn">manquante</Badge>
                      )}
                    </td>
                    <td onClick={(e) => e.stopPropagation()}>
                      <div className="row gap-4">
                        <button className="btn btn-ghost btn-sm btn-icon" title="Modifier" onClick={() => setEditing(p)}>
                          <Pencil size={14} />
                        </button>
                        <button className="btn btn-ghost btn-sm btn-icon" title="Supprimer" onClick={() => remove(p)}>
                          <Trash size={14} color="var(--err)" />
                        </button>
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      <div className="row gap-16 wrap" style={{ alignItems: "flex-start" }}>
        <Field label="Outils de lecture du cluster" hint="Le modèle peut lister les objets, lire un YAML, les évènements, les journaux et les métriques. Jamais d'écriture.">
          <label className="checkbox">
            <input type="checkbox" checked={general.toolsEnabled} onChange={(e) => setGeneral({ ...general, toolsEnabled: e.target.checked })} /> Activés
          </label>
        </Field>
        <Field label="Tours d'outils maximum par question">
          <input
            className="input"
            type="number"
            min={1}
            max={50}
            style={{ width: 90 }}
            value={general.maxToolRounds}
            onChange={(e) => setGeneral({ ...general, maxToolRounds: Math.max(1, Math.min(50, Number(e.target.value) || 1)) })}
          />
        </Field>
      </div>
      <Field label="Instructions supplémentaires" hint="Ajoutées au message système : langue, ton, conventions de votre équipe…">
        <textarea
          className="input"
          rows={3}
          value={general.extraInstructions}
          onChange={(e) => setGeneral({ ...general, extraInstructions: e.target.value })}
          placeholder="Ex. : Réponds en anglais. Nos namespaces de prod commencent par « prd- »."
        />
      </Field>
      <div className="row">
        <button className="btn btn-primary btn-sm" disabled={!dirty || saveGeneral.isPending} onClick={() => saveGeneral.mutate()}>
          Enregistrer
        </button>
      </div>

      <ProfileDialog
        open={editing !== null}
        account={claudeCode.data ?? null}
        profile={editing === "new" ? null : editing}
        onClose={() => setEditing(null)}
        onSaved={() => {
          setEditing(null);
          invalidate();
        }}
      />
    </Section>
  );
}

function ProfileDialog({
  open,
  profile,
  account,
  onClose,
  onSaved,
}: {
  open: boolean;
  profile: ProfileView | null;
  /** Compte de Claude Code détecté, s'il y en a un. */
  account: ClaudeCodeAccount | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const toast = useStore((s) => s.toast);
  const [preset, setPreset] = useState<string>("anthropic");
  const [form, setForm] = useState<ProfileUpdate>({ name: "", kind: "anthropic", baseUrl: "", apiKey: "", model: "", maxOutputTokens: null, showThinking: false, useClaudeCode: false });
  const [models, setModels] = useState<ModelInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setModels(null);
    setError(null);
    if (profile) {
      const p = PRESETS.find((x) => x.kind === profile.kind && x.baseUrl === profile.baseUrl) ?? PRESETS.find((x) => x.kind === profile.kind && x.id === "custom") ?? PRESETS[0]!;
      setPreset(p.id);
      setForm({ id: profile.id, name: profile.name, kind: profile.kind, baseUrl: profile.baseUrl, apiKey: undefined, model: profile.model, maxOutputTokens: profile.maxOutputTokens, showThinking: profile.showThinking, useClaudeCode: profile.useClaudeCode });
    } else {
      applyPreset(PRESETS[0]!);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, profile]);

  const applyPreset = (p: Preset) => {
    setPreset(p.id);
    setForm((f) => ({
      ...f,
      kind: p.kind,
      baseUrl: p.baseUrl,
      model: p.model,
      name: f.name || p.label,
      useClaudeCode: p.kind === "anthropic" ? f.useClaudeCode : false,
    }));
    setModels(null);
  };
  const currentPreset = PRESETS.find((p) => p.id === preset) ?? PRESETS[0]!;

  const list = useMutation({
    mutationFn: () => api.ai.listModels({ ...form, apiKey: form.apiKey || undefined }),
    onSuccess: (m) => {
      setModels(m);
      setError(null);
      if (m.length === 0) setError("Le fournisseur ne renvoie aucun modèle.");
      else if (!form.model || !m.some((x) => x.id === form.model)) setForm((f) => ({ ...f, model: f.model || m[0]!.id }));
    },
    onError: (e: Error) => setError(e.message),
  });

  const save = useMutation({
    mutationFn: () => api.ai.upsertProfile({ ...form, apiKey: form.apiKey === "" && profile ? undefined : form.apiKey }),
    onSuccess: () => {
      toast("ok", "Profil enregistré.");
      onSaved();
    },
    onError: (e: Error) => setError(e.message),
  });

  const borrowed = form.kind === "anthropic" && !!form.useClaudeCode;
  const needsKey = form.kind !== "openAiCompatible" && !borrowed && !profile?.apiKeySet && !form.apiKey;

  return (
    <Dialog
      open={open}
      title={profile ? `Modifier « ${profile.name} »` : "Nouveau fournisseur d'IA"}
      onClose={onClose}
      icon={<Sparkle size={20} />}
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Annuler
          </button>
          <button className="btn btn-primary" disabled={!form.name.trim() || needsKey || save.isPending} onClick={() => save.mutate()}>
            {save.isPending ? <Spinner /> : null} Enregistrer
          </button>
        </>
      }
    >
      <Field label="Type de fournisseur">
        <div className="row wrap">
          {PRESETS.map((p) => (
            <button key={p.id} className={`btn btn-sm ${preset === p.id ? "active btn-group" : ""}`} onClick={() => applyPreset(p)} style={preset === p.id ? { background: "var(--accent-soft)", color: "var(--accent)", borderColor: "var(--accent)" } : undefined}>
              {p.label}
            </button>
          ))}
        </div>
        <div className="hint">{currentPreset.hint}</div>
      </Field>
      <div className="row gap-12" style={{ alignItems: "flex-start" }}>
        <Field label="Nom">
          <input className="input" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
        </Field>
        <div className="grow">
          <Field label="Adresse de base">
            <input className="input mono" value={form.baseUrl ?? ""} onChange={(e) => setForm({ ...form, baseUrl: e.target.value })} placeholder={PRESETS.find((p) => p.kind === form.kind)?.baseUrl} />
          </Field>
        </div>
      </div>
      {form.kind === "anthropic" && (
        <Field
          label="Compte Claude Code"
          hint={
            account
              ? `Lu dans ${account.dir} (${account.source}) : ${accountLabel(account)}. Rien n'est recopié dans les réglages de KubeWatch.`
              : "Aucun dossier ~/.claude sur cette machine : installez Claude Code et connectez-vous pour l'employer."
          }
        >
          <label className="checkbox">
            <input
              type="checkbox"
              checked={borrowed}
              disabled={!account}
              onChange={(e) => setForm({ ...form, useClaudeCode: e.target.checked })}
            />{" "}
            Employer les identifiants de Claude Code, sans clé d'API
          </label>
        </Field>
      )}
      {!borrowed && (
        <Field
          label={form.kind === "openAiCompatible" ? "Clé d'API (facultative)" : "Clé d'API"}
          hint={profile?.apiKeySet ? "Une clé est enregistrée : laissez vide pour la conserver." : undefined}
        >
          <input
            className="input mono"
            type="password"
            value={form.apiKey ?? ""}
            onChange={(e) => setForm({ ...form, apiKey: e.target.value })}
            placeholder={profile?.apiKeySet ? "••••••••  (inchangée)" : form.kind === "anthropic" ? "sk-ant-…" : "sk-…"}
            autoComplete="off"
          />
        </Field>
      )}
      <Field label="Modèle" hint="Listez les modèles pour vérifier la connexion et choisir dans la liste.">
        <div className="row">
          {models && models.length > 0 ? (
            <select className="select grow mono" value={form.model} onChange={(e) => setForm({ ...form, model: e.target.value })}>
              {models.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.id}
                  {m.displayName ? ` — ${m.displayName}` : ""}
                </option>
              ))}
            </select>
          ) : (
            <input className="input grow mono" value={form.model} onChange={(e) => setForm({ ...form, model: e.target.value })} placeholder={form.kind === "openAiCompatible" ? "nom du modèle chargé" : ""} />
          )}
          <button className="btn" onClick={() => list.mutate()} disabled={list.isPending}>
            {list.isPending ? <Spinner /> : "Lister les modèles"}
          </button>
        </div>
      </Field>
      <div className="row gap-12" style={{ alignItems: "flex-start" }}>
        <Field label="Sortie maximale (jetons)" hint="Vide : défaut du fournisseur.">
          <input
            className="input"
            type="number"
            min={256}
            style={{ width: 130 }}
            value={form.maxOutputTokens ?? ""}
            onChange={(e) => setForm({ ...form, maxOutputTokens: e.target.value ? Number(e.target.value) : null })}
          />
        </Field>
        {form.kind === "anthropic" && (
          <Field label="Raisonnement" hint="Claude 4.6 et suivants : affiche un résumé du raisonnement.">
            <label className="checkbox">
              <input type="checkbox" checked={!!form.showThinking} onChange={(e) => setForm({ ...form, showThinking: e.target.checked })} /> Afficher
            </label>
          </Field>
        )}
      </div>
      {error && <Alert tone="err">{error}</Alert>}
      {models && models.length > 0 && !error && <Alert tone="ok">Connexion réussie : {models.length} modèle{models.length > 1 ? "s" : ""} disponible{models.length > 1 ? "s" : ""}.</Alert>}
    </Dialog>
  );
}

// --- Mises à jour ------------------------------------------------------------

const CHANNELS: { id: UpdateChannel; label: string }[] = [
  { id: "major", label: "Majeures" },
  { id: "minor", label: "Mineures" },
  { id: "patch", label: "Correctifs" },
  { id: "prerelease", label: "Préversions" },
  { id: "pinned", label: "Épinglé" },
];

function UpdaterSection() {
  const toast = useStore((s) => s.toast);
  const qc = useQueryClient();
  const q = useQuery({ queryKey: ["updater-settings"], queryFn: api.updates.settings });
  const [form, setForm] = useState<UpdaterSettings | null>(null);
  useEffect(() => {
    if (q.data) setForm(q.data);
  }, [q.data]);
  const save = useMutation({
    mutationFn: (s: UpdaterSettings) => api.updates.saveSettings(s),
    onSuccess: (s) => {
      setForm(s);
      qc.setQueryData(["updater-settings"], s);
      toast("ok", "Réglages des mises à jour enregistrés.");
    },
    onError: (e: Error) => toast("err", e.message),
  });
  if (!form) return null;
  return (
    <Section title="Mises à jour">
      <div className="row gap-12" style={{ alignItems: "flex-start" }}>
        <div className="grow">
          <Field label="Jeton GitHub" hint="Augmente le quota d'API et donne accès aux dépôts privés. Masqué une fois enregistré.">
            <input className="input mono" type="password" value={form.githubToken ?? ""} onChange={(e) => setForm({ ...form, githubToken: e.target.value || null })} autoComplete="off" />
          </Field>
        </div>
        <Field label="Canal par défaut">
          <select className="select" value={form.defaultPolicy.channel} onChange={(e) => setForm({ ...form, defaultPolicy: { ...form.defaultPolicy, channel: e.target.value as UpdateChannel } })}>
            {CHANNELS.map((c) => (
              <option key={c.id} value={c.id}>
                {c.label}
              </option>
            ))}
          </select>
        </Field>
      </div>
      <div className="row gap-12" style={{ alignItems: "flex-start" }}>
        <div className="grow">
          <Field label="Secret de webhook GitHub" hint="Sert à vérifier la signature des webhooks ; sans usage dans l'application de bureau, conservé pour compatibilité.">
            <input className="input mono" type="password" value={form.webhookSecret ?? ""} onChange={(e) => setForm({ ...form, webhookSecret: e.target.value || null })} autoComplete="off" />
          </Field>
        </div>
        <Field label="Intervalle de vérification" hint={`${humanInterval(form.defaultPolicy.checkIntervalSeconds)} (nouveaux surveillants)`}>
          <input
            className="input"
            type="number"
            min={60}
            step={60}
            style={{ width: 130 }}
            value={form.defaultPolicy.checkIntervalSeconds}
            onChange={(e) => setForm({ ...form, defaultPolicy: { ...form.defaultPolicy, checkIntervalSeconds: Math.max(60, Number(e.target.value) || 60) } })}
          />
        </Field>
        <Field label="Contrainte semver" hint="Ex. <2.0.0 ; vide = aucune.">
          <input className="input mono" style={{ width: 140 }} value={form.defaultPolicy.constraint ?? ""} onChange={(e) => setForm({ ...form, defaultPolicy: { ...form.defaultPolicy, constraint: e.target.value || null } })} />
        </Field>
      </div>
      <div className="row gap-16 wrap">
        <label className="checkbox" title="Réglage conservé dans le fichier d'état pour un usage futur : aucune vérification périodique n'est lancée aujourd'hui.">
          <input type="checkbox" checked={form.schedulerEnabled} onChange={(e) => setForm({ ...form, schedulerEnabled: e.target.checked })} /> Autoriser la vérification périodique <span className="muted">(sans effet : non implémentée)</span>
        </label>
        <label className="checkbox">
          <input type="checkbox" checked={form.defaultPolicy.allowPrerelease} onChange={(e) => setForm({ ...form, defaultPolicy: { ...form.defaultPolicy, allowPrerelease: e.target.checked } })} /> Accepter les préversions
        </label>
        <label className="checkbox">
          <input type="checkbox" checked={form.defaultPolicy.autoApply} onChange={(e) => setForm({ ...form, defaultPolicy: { ...form.defaultPolicy, autoApply: e.target.checked } })} /> Appliquer automatiquement (nouveaux surveillants)
        </label>
      </div>
      {form.defaultPolicy.autoApply && (
        <Alert tone="warn">
          L'application automatique marque les surveillants comme déployables sans confirmation. Elle ne s'exécute que
          pendant une vérification, et aucune vérification n'a lieu d'elle-même : c'est « Vérifier maintenant », dans
          l'écran Mises à jour, qui déclenche tout.
        </Alert>
      )}
      <div className="row">
        <button className="btn btn-primary btn-sm" disabled={save.isPending} onClick={() => save.mutate(form)}>
          Enregistrer
        </button>
      </div>
    </Section>
  );
}

function humanInterval(seconds: number): string {
  if (seconds % 86400 === 0) return `toutes les ${seconds / 86400} j`;
  if (seconds % 3600 === 0) return `toutes les ${seconds / 3600} h`;
  if (seconds % 60 === 0) return `toutes les ${seconds / 60} min`;
  return `toutes les ${seconds} s`;
}

function AboutSection() {
  const info = useQuery({ queryKey: ["app-info-static"], queryFn: api.appInfo, staleTime: Infinity });
  return (
    <Section title="À propos">
      <div className="col gap-4 small">
        <div>
          <strong>KubeWatch</strong> {info.data?.version} — application de bureau Tauri (interface web) et cœur Rust.
        </div>
        <div className="muted">
          Dossier d'état : <span className="mono selectable">{info.data?.stateDir}</span>
        </div>
        <div>
          <a href="#" onClick={(e) => { e.preventDefault(); api.openUrl("https://github.com/kubewatch-io/kubewatch"); }}>
            Dépôt du projet
          </a>
        </div>
      </div>
    </Section>
  );
}
