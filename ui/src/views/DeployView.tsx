// Écran « Déployer » : l'assistant de déploiement en quatre étapes — quoi,
// comment, vérification, résultat. Port de crates/desktop/src/views/deploy.rs.
// La logique pure (modèle, validation, prévision de consommation) vit dans
// ./deploy/*.ts ; ce fichier ne contient que l'affichage et les appels au backend.
import {
  ArrowLeft,
  ArrowRight,
  ArrowSquareOut,
  CheckCircle,
  Copy,
  Cube,
  Flask,
  Info,
  MagnifyingGlass,
  Package,
  Plus,
  Prohibit,
  RocketLaunch,
  Warning,
  X,
  XCircle,
} from "@phosphor-icons/react";
import { useMutation, useQuery, useQueryClient, type UseQueryResult } from "@tanstack/react-query";
import { Fragment, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { api } from "@/api/client";
import type { CatalogApp, DeployRequest, ImageDetails } from "@/api/hub";
import type { ApplyOutcome, ClusterOverview } from "@/api/types";
import { useNamespaces, useOverview } from "@/app/queries";
import { useNamespace, useStore } from "@/app/store";
import { Alert, Badge, EmptyState, Section, Spinner } from "@/components/Basics";
import { confirm } from "@/components/Confirm";
import { YamlEditor } from "@/components/YamlEditor";
import { bytes, clip, millicores } from "@/lib/format";
import {
  applyDraft,
  buildRequest,
  EXPOSURE_ORDER,
  EXPOSURES,
  furthest,
  newWizard,
  PROFILE_ORDER,
  PROFILES,
  prefillFromCatalog,
  prefillFromImage,
  readDraft,
  restart,
  STEPS,
  stepIndex,
  type DeployDraft,
  type Step,
  type Wizard,
} from "./deploy/model";
import {
  afterFraction,
  afterRatio,
  assess,
  beforeFraction,
  FIT_HEADLINE,
  remaining,
  type Assessment,
  type Axis,
  type Fit,
  type Note,
} from "./deploy/sizing";
import { isDns1123Label, parseImageRef, validate } from "./deploy/validate";
import "./Deploy.css";

type Patch = Partial<Wizard> | ((w: Wizard) => Partial<Wizard>);
type PatchFn = (p: Patch) => void;

const ACTION_TONE = { created: "ok", configured: "info", unchanged: "neutral", dryRun: "info", deleted: "warn", failed: "err" } as const;
const ACTION_LABEL = { created: "créé", configured: "configuré", unchanged: "inchangé", dryRun: "simulé", deleted: "supprimé", failed: "échec" } as const;

const FIT_ICON = { fits: CheckCircle, tight: Warning, exceeds: XCircle, unknown: Info } as const;
const NOTE_ICON = { danger: Prohibit, warning: Warning, info: Info } as const;

/** Entier borné depuis la saisie d'un champ numérique ; une saisie vide ou illisible retombe sur le minimum. */
function clampInt(raw: string, min: number, max: number): number {
  const n = Math.trunc(Number(raw));
  if (!Number.isFinite(n)) return min;
  return Math.max(min, Math.min(max, n));
}

// ---------------------------------------------------------------------------
// Point d'entrée
// ---------------------------------------------------------------------------

export function DeployView() {
  const cluster = useStore((s) => s.cluster);
  const goToResource = useStore((s) => s.goToResource);
  const currentNamespace = useNamespace();
  const qc = useQueryClient();

  const [w, setW] = useState<Wizard>(() => newWizard(currentNamespace ?? "default"));
  const patch = useCallback<PatchFn>((p) => setW((prev) => ({ ...prev, ...(typeof p === "function" ? p(prev) : p) })), []);
  const go = useCallback((step: Step) => setW((prev) => ({ ...prev, step, reached: furthest(prev.reached, step) })), []);

  // Le catalogue est embarqué dans le binaire : une seule lecture par session.
  const catalog = useQuery({ queryKey: ["hub-catalog"], queryFn: api.hub.catalog, staleTime: Infinity });
  const namespaces = useNamespaces(cluster);
  const overview = useOverview(cluster);

  const [showYaml, setShowYaml] = useState(false);
  const [outcome, setOutcome] = useState<{ dryRun: boolean; value: ApplyOutcome } | null>(null);
  const [attemptDryRun, setAttemptDryRun] = useState(false);

  // Brouillon déposé par l'écran Hub : appliqué au montage depuis l'image, puis
  // affiné dès que le catalogue est là si l'application y figure.
  const draftRef = useRef<DeployDraft | null>(null);
  useEffect(() => {
    const draft = readDraft();
    if (!draft) return;
    if (draft.catalogAppId) draftRef.current = draft;
    setW((prev) => applyDraft(prev, draft, null));
  }, []);
  useEffect(() => {
    const draft = draftRef.current;
    if (!draft || !catalog.data) return;
    draftRef.current = null;
    const app = catalog.data.find((a) => a.id === draft.catalogAppId);
    if (app) setW((prev) => applyDraft(prev, draft, app));
  }, [catalog.data]);

  const request = useMemo(() => buildRequest(w), [w]);
  const error = useMemo(() => validate(request), [request]);
  const assessment = useMemo(() => assess(request, overview.data), [request, overview.data]);

  // Le manifeste est demandé à l'entrée dans la vérification, pour la requête
  // telle quelle ; la clé porte la requête entière, un rendu en échec ne
  // relance donc rien tant qu'elle ne change pas.
  const render = useQuery({
    queryKey: ["hub-render", request],
    queryFn: () => api.hub.render(request),
    enabled: w.step === "preview" && !error,
    staleTime: Infinity,
  });

  const deploy = useMutation({
    mutationFn: ({ target, dryRun }: { target: string; dryRun: boolean }) => api.hub.deploy(target, request, dryRun),
    onSuccess: (value, { target, dryRun }) => {
      setOutcome({ dryRun, value });
      if (!dryRun) {
        qc.invalidateQueries({ queryKey: ["overview", target] });
        qc.invalidateQueries({ queryKey: ["resources", target] });
      }
    },
  });

  const busy = render.isFetching || deploy.isPending;
  const ready = !!w.name.trim() && !!w.image.trim();
  const canDeploy = !error && !!cluster && !busy;

  const pick = (next: (prev: Wizard) => Wizard) => {
    setW((prev) => ({ ...next(prev), step: "how", reached: "how" }));
    setOutcome(null);
    deploy.reset();
  };

  const launch = async (dryRun: boolean) => {
    if (!cluster) return;
    if (!dryRun) {
      const ok = await confirm({
        title: `Déployer « ${request.name} » ?`,
        message: `Les manifestes seront appliqués côté serveur sur « ${cluster} », dans le namespace « ${request.namespace} ».`,
        confirmLabel: "Déployer",
      });
      if (!ok) return;
    }
    setOutcome(null);
    setAttemptDryRun(dryRun);
    go("result");
    deploy.mutate({ target: cluster, dryRun });
  };

  const again = () => {
    setW(restart);
    setOutcome(null);
    setShowYaml(false);
    deploy.reset();
  };

  return (
    <div className="deploy">
      <div className="deploy-head">
        <div className="page-header">
          <h1>Déployer</h1>
          <span className="grow" />
          {cluster ? (
            <span className="small muted">
              Cluster « <strong>{cluster}</strong> »
            </span>
          ) : (
            <Badge tone="warn">Aucun cluster sélectionné</Badge>
          )}
        </div>
        <StepBar step={w.step} reached={w.reached} onGo={go} />
      </div>

      <div className="deploy-body">
        {w.step === "what" && (
          <StepWhat
            w={w}
            patch={patch}
            catalog={catalog}
            onPickApp={(app) => pick((prev) => prefillFromCatalog(prev, app))}
            onUseImage={(reference, ports) => pick((prev) => prefillFromImage(prev, reference, ports))}
          />
        )}
        {w.step === "how" && <StepHow w={w} patch={patch} namespaces={namespaces.data ?? []} />}
        {w.step === "preview" && (
          <StepPreview
            request={request}
            error={error}
            assessment={assessment}
            overview={overview.data ?? null}
            render={render}
            showYaml={showYaml}
            setShowYaml={setShowYaml}
          />
        )}
        {w.step === "result" && (
          <StepResult
            outcome={outcome}
            pending={deploy.isPending}
            error={deploy.error}
            dryRun={attemptDryRun}
            onView={(name, namespace) => goToResource({ kind: "deployments", name, namespace })}
          />
        )}
      </div>

      <div className="deploy-foot">
        {w.step === "how" && (
          <button className="btn" onClick={() => go("what")}>
            <ArrowLeft size={14} /> Changer d'application
          </button>
        )}
        {(w.step === "preview" || w.step === "result") && (
          <button className="btn" onClick={() => go("how")}>
            <ArrowLeft size={14} /> Modifier les réglages
          </button>
        )}
        <span className="grow" />
        {busy && <Spinner />}
        {!cluster && w.step !== "what" && (
          <span className="small" style={{ color: "var(--warn)" }}>
            Aucun cluster sélectionné.
          </span>
        )}
        {w.step === "what" && <span className="small muted">Choisissez une application pour continuer.</span>}
        {w.step === "how" && (
          <button className="btn btn-primary" disabled={!ready} onClick={() => go("preview")}>
            Vérifier <ArrowRight size={14} />
          </button>
        )}
        {w.step === "preview" && (
          <>
            <button
              className="btn"
              disabled={!canDeploy}
              title="Envoie les manifestes au serveur d'API en mode « dry-run » : il les valide sans rien écrire."
              onClick={() => launch(true)}
            >
              <Flask size={14} /> Simuler d'abord
            </button>
            <button className="btn btn-primary" disabled={!canDeploy} onClick={() => launch(false)}>
              <RocketLaunch size={14} /> Déployer
            </button>
          </>
        )}
        {w.step === "result" && (
          <>
            {outcome?.dryRun && (
              <button className="btn btn-primary" disabled={!cluster || busy} onClick={() => launch(false)}>
                <RocketLaunch size={14} /> Déployer pour de bon
              </button>
            )}
            <button className="btn" onClick={again}>
              Déployer autre chose
            </button>
          </>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Fil d'étapes
// ---------------------------------------------------------------------------

function StepBar({ step, reached, onGo }: { step: Step; reached: Step; onGo: (s: Step) => void }) {
  return (
    <nav className="deploy-steps" aria-label="Étapes de l'assistant">
      {STEPS.map((s, i) => {
        const idx = stepIndex(s.id);
        const accessible = idx <= stepIndex(reached);
        const cls = s.id === step ? "active" : idx < stepIndex(step) ? "done" : "";
        return (
          <Fragment key={s.id}>
            {i > 0 && (
              <span className="deploy-steps-sep" aria-hidden="true">
                ›
              </span>
            )}
            <button
              type="button"
              className={`deploy-step ${cls}`}
              disabled={!accessible}
              aria-current={s.id === step ? "step" : undefined}
              onClick={() => s.id !== step && onGo(s.id)}
            >
              <span className="deploy-step-num">{i + 1}</span>
              {s.label}
            </button>
          </Fragment>
        );
      })}
    </nav>
  );
}

// ---------------------------------------------------------------------------
// Étape 1 — quoi déployer
// ---------------------------------------------------------------------------

function StepWhat({
  w,
  patch,
  catalog,
  onPickApp,
  onUseImage,
}: {
  w: Wizard;
  patch: PatchFn;
  catalog: UseQueryResult<CatalogApp[]>;
  onPickApp: (app: CatalogApp) => void;
  onUseImage: (reference: string, ports: number[]) => void;
}) {
  return (
    <>
      <div className="deploy-intro">
        <h2>Que voulez-vous déployer ?</h2>
        <p className="muted">Choisissez une application prête à l'emploi, ou indiquez l'image de la vôtre.</p>
      </div>
      <div>
        <div className="btn-group">
          <button className={`btn ${w.source === "catalog" ? "active" : ""}`} onClick={() => patch({ source: "catalog" })}>
            <Package size={14} /> Catalogue
          </button>
          <button className={`btn ${w.source === "image" ? "active" : ""}`} onClick={() => patch({ source: "image" })}>
            <Cube size={14} /> Mon image
          </button>
        </div>
      </div>
      {w.source === "catalog" ? <CatalogGrid w={w} patch={patch} catalog={catalog} onPick={onPickApp} /> : <ImageField w={w} patch={patch} onUse={onUseImage} />}
    </>
  );
}

/** Grille des applications du catalogue, filtrable par texte et par catégorie. */
function CatalogGrid({
  w,
  patch,
  catalog,
  onPick,
}: {
  w: Wizard;
  patch: PatchFn;
  catalog: UseQueryResult<CatalogApp[]>;
  onPick: (app: CatalogApp) => void;
}) {
  if (catalog.isPending) return <Spinner label="Chargement du catalogue…" />;
  if (catalog.isError) return <Alert tone="err">{catalog.error.message}</Alert>;
  const apps = catalog.data;
  if (apps.length === 0) return <EmptyState icon={Package} title="Catalogue vide" hint="Aucune application embarquée ; indiquez votre propre image." />;

  const categories = [...new Set(apps.map((a) => a.category))].sort((a, b) => a.localeCompare(b, "fr"));
  const filter = w.filter.trim().toLowerCase();
  const kept = apps.filter(
    (a) =>
      (w.category == null || a.category === w.category) &&
      (!filter || [a.name, a.description, a.category, a.image].some((s) => s.toLowerCase().includes(filter))),
  );

  return (
    <>
      <div className="row wrap">
        <div className="search" style={{ width: 260 }}>
          <MagnifyingGlass size={14} />
          <input
            className="input"
            value={w.filter}
            onChange={(e) => patch({ filter: e.target.value })}
            placeholder="Filtrer : base de données, grafana…"
            aria-label="Filtrer le catalogue"
          />
        </div>
        <span className="vdivider" style={{ height: 20 }} />
        <button className={`deploy-chip ${w.category == null ? "active" : ""}`} onClick={() => patch({ category: null })}>
          Toutes
        </button>
        {categories.map((c) => (
          <button key={c} className={`deploy-chip ${w.category === c ? "active" : ""}`} onClick={() => patch({ category: w.category === c ? null : c })}>
            {c}
          </button>
        ))}
      </div>
      {kept.length === 0 ? (
        <span className="small muted">Aucune application ne correspond à ce filtre.</span>
      ) : (
        <div className="deploy-grid">
          {kept.map((app) => (
            <AppCard key={app.id} app={app} selected={w.picked === app.id} onClick={() => onPick(app)} />
          ))}
        </div>
      )}
    </>
  );
}

function AppCard({ app, selected, onClick }: { app: CatalogApp; selected: boolean; onClick: () => void }) {
  return (
    <button type="button" className={`deploy-card ${selected ? "selected" : ""}`} onClick={onClick} aria-pressed={selected}>
      <div className="row" style={{ alignSelf: "stretch" }}>
        <Pastille name={app.name} />
        <div className="col grow" style={{ gap: 0 }}>
          <strong className="truncate">{app.name}</strong>
          <span className="xs muted">{app.category}</span>
        </div>
      </div>
      <span className="small deploy-card-desc">{clip(app.description, 110)}</span>
      <div className="row wrap gap-4">
        {app.defaultPort != null && <span className="tag">port {app.defaultPort}</span>}
        {app.needsPvc && <span className="tag">volume</span>}
        {app.env.length > 0 && (
          <span className="tag">
            {app.env.length} réglage{app.env.length > 1 ? "s" : ""}
          </span>
        )}
      </div>
    </button>
  );
}

/** Teinte reproductible tirée d'un nom : la même application garde sa couleur. */
function stableHue(name: string): number {
  let h = 2166136261;
  for (let i = 0; i < name.length; i++) {
    h ^= name.charCodeAt(i) & 0xff;
    h = Math.imul(h, 16777619) >>> 0;
  }
  return h % 360;
}

/** Pastille colorée portant l'initiale de l'application, hors ligne et sans dépendance. */
function Pastille({ name }: { name: string }) {
  const initial = name.trim().charAt(0).toUpperCase() || "?";
  return (
    <span className="deploy-pastille" style={{ "--hue": stableHue(name) } as CSSProperties} aria-hidden="true">
      {initial}
    </span>
  );
}

/** Saisie d'une référence d'image, avec lecture facultative de ses métadonnées. */
function ImageField({ w, patch, onUse }: { w: Wizard; patch: PatchFn; onUse: (reference: string, ports: number[]) => void }) {
  const toast = useStore((s) => s.toast);
  const [details, setDetails] = useState<ImageDetails | null>(null);
  const inspect = useMutation({
    mutationFn: (image: string) => api.hub.inspect(image),
    onSuccess: (d) => setDetails(d),
    onError: (e: Error) => toast("err", e.message, "Lecture de l'image"),
  });

  const input = w.imageInput.trim();
  const parsed = parseImageRef(input);
  // La dernière inspection ne sert que si elle concerne bien l'image saisie.
  const shown = details && parsed.ok && details.reference === parsed.full ? details : null;
  const proceed = () => {
    if (parsed.ok) onUse(input, shown?.exposedPorts ?? []);
  };

  return (
    <div className="col gap-8">
      <div className="row wrap">
        <input
          className="input mono"
          style={{ width: 360 }}
          value={w.imageInput}
          onChange={(e) => patch({ imageInput: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === "Enter") proceed();
          }}
          placeholder="ghcr.io/mon-org/mon-app:1.4.0"
          aria-label="Référence d'image"
          autoFocus
        />
        <button
          className="btn"
          disabled={!input || inspect.isPending}
          title="Interroge le registre pour récupérer les ports exposés et les variables d'environnement par défaut de l'image."
          onClick={() => inspect.mutate(input)}
        >
          {inspect.isPending ? <Spinner /> : <MagnifyingGlass size={14} />} Lire l'image
        </button>
      </div>
      {!input ? (
        <span className="small muted">Une référence complète : registre, dépôt et version.</span>
      ) : parsed.ok ? (
        <span className="small row gap-4" style={{ color: "var(--ok)" }}>
          <CheckCircle size={14} /> <span className="mono">{parsed.full}</span>
        </span>
      ) : (
        <span className="small row gap-4" style={{ color: "var(--err)" }}>
          <Warning size={14} /> {parsed.error}
        </span>
      )}
      {shown && (
        <div className="card" style={{ alignSelf: "flex-start", minWidth: 320 }}>
          <div className="card-body col gap-4">
            <strong>Ce que dit l'image</strong>
            <span className="small">Ports : {shown.exposedPorts.length ? shown.exposedPorts.join(", ") : "aucun port déclaré"}</span>
            <span className="small">Variables par défaut : {shown.env.length}</span>
            {shown.architecture && (
              <span className="small">
                Plate-forme : {shown.os ?? "linux"}/{shown.architecture}
              </span>
            )}
          </div>
        </div>
      )}
      <div>
        <button className="btn btn-primary" disabled={!parsed.ok} onClick={proceed}>
          Continuer avec cette image <ArrowRight size={14} />
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Étape 2 — comment le déployer
// ---------------------------------------------------------------------------

function StepHow({ w, patch, namespaces }: { w: Wizard; patch: PatchFn; namespaces: string[] }) {
  return (
    <>
      <div className="deploy-intro">
        <h2>Comment le déployer ?</h2>
        <p className="muted">Les valeurs proposées conviennent dans la plupart des cas.</p>
      </div>
      <IdentityBlock w={w} patch={patch} namespaces={namespaces} />
      <SizeBlock w={w} patch={patch} />
      <AccessBlock w={w} patch={patch} />
      <StorageBlock w={w} patch={patch} />
      <Section title="Réglages de l'application">
        <PairsEditor
          pairs={w.env}
          onChange={(env) => patch({ env })}
          keyPlaceholder="CLÉ"
          valuePlaceholder="valeur"
          addLabel="Ajouter une variable"
          emptyText="Aucune variable d'environnement."
        />
      </Section>
      <Section title="Étiquettes">
        <span className="xs muted">
          Ajoutées à tous les objets créés, en plus de celles que KubeWatch pose (app.kubernetes.io/name, instance, managed-by).
        </span>
        <PairsEditor
          pairs={w.labels}
          onChange={(labels) => patch({ labels })}
          keyPlaceholder="equipe"
          valuePlaceholder="paiement"
          addLabel="Ajouter une étiquette"
          emptyText="Aucune étiquette supplémentaire."
        />
      </Section>
    </>
  );
}

/** Nom, namespace et nombre de répliques. */
function IdentityBlock({ w, patch, namespaces }: { w: Wizard; patch: PatchFn; namespaces: string[] }) {
  const nameOk = isDns1123Label(w.name.trim());
  const replicasText = w.replicas === 0 ? "aucun pod ne démarrera" : w.replicas === 1 ? "une copie de l'application" : "copies réparties sur les nœuds";
  return (
    <Section title="Identité">
      <div className="deploy-form">
        <label htmlFor="deploy-name">Nom</label>
        <div className="col gap-4">
          <input id="deploy-name" className="input" style={{ width: 260 }} value={w.name} onChange={(e) => patch({ name: e.target.value })} placeholder="mon-api" />
          {!nameOk && (
            <span className="xs" style={{ color: "var(--warn)" }}>
              Minuscules, chiffres et tirets : « mon-api », « site-vitrine ».
            </span>
          )}
        </div>

        <label htmlFor="deploy-namespace">Namespace</label>
        <div>
          <input
            id="deploy-namespace"
            className="input"
            style={{ width: 200 }}
            list="deploy-namespaces"
            value={w.namespace}
            onChange={(e) => patch({ namespace: e.target.value })}
            placeholder="default"
          />
          <datalist id="deploy-namespaces">
            {namespaces.map((n) => (
              <option key={n} value={n} />
            ))}
          </datalist>
        </div>

        <label>Image</label>
        <span className="mono selectable">{w.image}</span>

        <label htmlFor="deploy-replicas">Répliques</label>
        <div className="row wrap">
          {/* Le curseur borne ce qu'il affiche : au-delà de dix répliques, le compteur les préserve. */}
          <input
            type="range"
            className="deploy-range"
            min={0}
            max={10}
            value={Math.min(10, w.replicas)}
            onChange={(e) => patch({ replicas: Number(e.target.value) })}
            aria-label="Répliques (curseur)"
          />
          <input
            id="deploy-replicas"
            type="number"
            className="input input-sm"
            style={{ width: 80 }}
            min={0}
            max={1000}
            value={w.replicas}
            onChange={(e) => patch({ replicas: clampInt(e.target.value, 0, 1000) })}
          />
          <span className="small muted">{replicasText}</span>
        </div>
      </div>
    </Section>
  );
}

/** Choix du profil de taille, sous forme de cartes chiffrées. */
function SizeBlock({ w, patch }: { w: Wizard; patch: PatchFn }) {
  return (
    <Section title="Taille">
      <span className="small muted">La demande est réservée par l'ordonnanceur ; la limite est le plafond que l'application ne dépassera pas.</span>
      <div className="deploy-grid deploy-grid-sm" role="radiogroup" aria-label="Profil de taille">
        {PROFILE_ORDER.map((id) => {
          const p = PROFILES[id];
          const on = w.profile === id;
          return (
            <button key={id} type="button" role="radio" aria-checked={on} className={`deploy-card ${on ? "selected" : ""}`} onClick={() => patch({ profile: id })}>
              <strong>{p.label}</strong>
              <span className="mono small">
                {p.cpuRequest} CPU · {p.memoryRequest}
              </span>
              <span className="xs muted">
                jusqu'à {p.cpuLimit} CPU · {p.memoryLimit}
              </span>
              <span className="xs muted deploy-card-desc">{p.description}</span>
            </button>
          );
        })}
      </div>
    </Section>
  );
}

/** Choix de l'exposition réseau et des ports. */
function AccessBlock({ w, patch }: { w: Wizard; patch: PatchFn }) {
  const setPort = (i: number, raw: string) =>
    patch((prev) => ({ ports: prev.ports.map((p, j) => (j === i ? { ...p, containerPort: clampInt(raw, 0, 65535) } : p)) }));
  const removePort = (i: number) => patch((prev) => ({ ports: prev.ports.filter((_, j) => j !== i) }));
  const addPort = () => patch((prev) => ({ ports: [...prev.ports, { containerPort: 8080 }] }));

  return (
    <Section title="Accès">
      <div className="deploy-radios" role="radiogroup" aria-label="Mode d'accès">
        {EXPOSURE_ORDER.map((id) => {
          const x = EXPOSURES[id];
          const on = w.exposure === id;
          return (
            <label key={id} className={`deploy-radio ${on ? "selected" : ""}`}>
              <input type="radio" name="deploy-exposure" checked={on} onChange={() => patch({ exposure: id })} />
              <span className="col" style={{ gap: 2 }}>
                <span>{x.label}</span>
                <span className="xs muted">{x.description}</span>
              </span>
            </label>
          );
        })}
      </div>

      {w.exposure === "domain" && (
        <div className="deploy-form">
          <label htmlFor="deploy-host">Domaine</label>
          <input id="deploy-host" className="input mono" style={{ width: 280 }} value={w.host} onChange={(e) => patch({ host: e.target.value })} placeholder="mon-app.exemple.fr" />
          <label htmlFor="deploy-ingress-class">Classe d'Ingress</label>
          <input
            id="deploy-ingress-class"
            className="input"
            style={{ width: 280 }}
            value={w.ingressClass}
            onChange={(e) => patch({ ingressClass: e.target.value })}
            placeholder="nginx, traefik… (facultatif)"
          />
          <label htmlFor="deploy-tls">Secret TLS</label>
          <input
            id="deploy-tls"
            className="input"
            style={{ width: 280 }}
            value={w.ingressTlsSecret}
            onChange={(e) => patch({ ingressTlsSecret: e.target.value })}
            placeholder="secret TLS existant (facultatif)"
          />
        </div>
      )}

      <div className="col gap-4">
        <strong className="small">Ports</strong>
        {w.ports.map((p, i) => (
          <div key={i} className="row">
            <span className="small muted">Le conteneur écoute sur</span>
            <input
              type="number"
              className="input input-sm mono"
              style={{ width: 96 }}
              min={1}
              max={65535}
              value={p.containerPort || ""}
              onChange={(e) => setPort(i, e.target.value)}
              aria-label={`Port ${i + 1}`}
            />
            <button className="btn btn-ghost btn-sm btn-icon" title="Retirer ce port" onClick={() => removePort(i)}>
              <X size={14} />
            </button>
          </div>
        ))}
        <div>
          <button className="btn btn-sm" onClick={addPort}>
            <Plus size={12} /> Ajouter un port
          </button>
        </div>
        {w.ports.length === 0 && (
          <span className="xs" style={{ color: "var(--warn)" }}>
            Sans port, aucun Service n'est créé : l'application ne sera pas joignable.
          </span>
        )}
      </div>
    </Section>
  );
}

/** Volume persistant. */
function StorageBlock({ w, patch }: { w: Wizard; patch: PatchFn }) {
  return (
    <Section title="Stockage">
      <label className="checkbox">
        <input type="checkbox" checked={w.storage} onChange={(e) => patch({ storage: e.target.checked })} /> Conserver les données entre les redémarrages
      </label>
      <span className="xs muted">Sans volume, tout ce que l'application écrit disparaît à chaque redémarrage du pod.</span>
      {w.storage && (
        <div className="deploy-form">
          <label htmlFor="deploy-size">Taille</label>
          <div className="row">
            <input
              id="deploy-size"
              type="number"
              className="input input-sm"
              style={{ width: 90 }}
              min={1}
              max={100000}
              value={w.storageGib}
              onChange={(e) => patch({ storageGib: clampInt(e.target.value, 1, 100000) })}
            />
            <span className="small muted">Gio</span>
          </div>
          <label htmlFor="deploy-mount">Monté dans</label>
          <input id="deploy-mount" className="input mono" style={{ width: 280 }} value={w.storagePath} onChange={(e) => patch({ storagePath: e.target.value })} placeholder="/data" />
          <label htmlFor="deploy-storage-class">Classe de stockage</label>
          <input
            id="deploy-storage-class"
            className="input"
            style={{ width: 280 }}
            value={w.storageClass}
            onChange={(e) => patch({ storageClass: e.target.value })}
            placeholder="celle du cluster (facultatif)"
          />
        </div>
      )}
    </Section>
  );
}

/** Éditeur de couples clé/valeur : variables d'environnement, étiquettes. */
function PairsEditor({
  pairs,
  onChange,
  keyPlaceholder,
  valuePlaceholder,
  addLabel,
  emptyText,
}: {
  pairs: [string, string][];
  onChange: (pairs: [string, string][]) => void;
  keyPlaceholder: string;
  valuePlaceholder: string;
  addLabel: string;
  emptyText: string;
}) {
  const set = (i: number, which: 0 | 1, value: string) =>
    onChange(pairs.map((p, j): [string, string] => (j !== i ? p : which === 0 ? [value, p[1]] : [p[0], value])));
  return (
    <div className="col gap-4">
      {pairs.length === 0 && <span className="xs muted">{emptyText}</span>}
      {pairs.map(([k, v], i) => (
        <div key={i} className="deploy-kv">
          <input className="input input-sm mono" value={k} placeholder={keyPlaceholder} onChange={(e) => set(i, 0, e.target.value)} aria-label="Clé" />
          <input className="input input-sm" value={v} placeholder={valuePlaceholder} onChange={(e) => set(i, 1, e.target.value)} aria-label="Valeur" />
          <button className="btn btn-ghost btn-sm btn-icon" title="Retirer" onClick={() => onChange(pairs.filter((_, j) => j !== i))}>
            <X size={14} />
          </button>
        </div>
      ))}
      <div>
        <button className="btn btn-sm" onClick={() => onChange([...pairs, ["", ""]])}>
          <Plus size={12} /> {addLabel}
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Étape 3 — vérification
// ---------------------------------------------------------------------------

function StepPreview({
  request,
  error,
  assessment,
  overview,
  render,
  showYaml,
  setShowYaml,
}: {
  request: DeployRequest;
  error: string | null;
  assessment: Assessment;
  overview: ClusterOverview | null;
  render: UseQueryResult<string>;
  showYaml: boolean;
  setShowYaml: (v: boolean) => void;
}) {
  const toast = useStore((s) => s.toast);
  const name = request.name || "—";
  const ports = request.ports ?? [];
  const host = request.ingressHost?.trim();
  const { cpu, memory, fit, notes, footprint } = assessment;
  const FitIcon = FIT_ICON[fit];

  // L'API agrégée peut être enregistrée et pourtant muette : le message ne présume pas de la cause.
  const reason = !overview
    ? "Le cluster n'a pas encore répondu : ouvrez la vue d'ensemble ou rafraîchissez."
    : !overview.metricsAvailable
      ? "Les mesures de consommation ne sont pas disponibles sur ce cluster (metrics-server absent, arrêté ou non prêt). Le déploiement reste possible."
      : "Capacité du cluster inconnue.";

  const copy = () => {
    if (!render.data) return;
    navigator.clipboard.writeText(render.data).then(
      () => toast("ok", "Manifeste copié."),
      () => toast("err", "Copie impossible."),
    );
  };

  const yamlStatus = render.isFetching ? (
    <Spinner label="Génération en cours…" />
  ) : render.isError ? (
    <Alert tone="err">{render.error.message}</Alert>
  ) : render.data ? (
    <span className="small muted">
      {render.data.split("\n").length} lignes, {render.data.split(/^---$/m).filter((d) => d.trim()).length} document(s).
    </span>
  ) : (
    <span className="small muted">Aucun manifeste : corrigez les réglages signalés, puis revenez ici.</span>
  );

  return (
    <>
      <div className="deploy-intro">
        <h2>Avant de déployer</h2>
      </div>

      {error && (
        <Alert tone="err">
          <Warning size={16} style={{ flex: "none", marginTop: 1 }} />
          <div className="col gap-4">
            <span>{error}</span>
            <span className="xs">Revenez à l'étape précédente pour corriger.</span>
          </div>
        </Alert>
      )}

      <Section title="Ce qui sera créé">
        <ul className="deploy-objects">
          <ObjectRow kind="Deployment" name={name} detail={`${Math.max(0, request.replicas ?? 1)} pod(s) de ${request.image}`} />
          {ports.length > 0 && (
            <ObjectRow kind="Service" name={name} detail={`${request.serviceType ?? "ClusterIP"}, port(s) ${ports.map((p) => p.containerPort).join(", ")}`} />
          )}
          {host && <ObjectRow kind="Ingress" name={name} detail={`https://${host}`} />}
          {request.pvc && <ObjectRow kind="PersistentVolumeClaim" name={`${name}-data`} detail={`${request.pvc.size} monté dans ${request.pvc.mountPath}`} />}
        </ul>
        <span className="small muted">Dans le namespace « {request.namespace} ».</span>
      </Section>

      <Section title="Consommation prévue">
        <div className={`row deploy-verdict deploy-fit-${fit}`}>
          <FitIcon size={18} weight="fill" />
          <strong>{FIT_HEADLINE[fit]}</strong>
        </div>
        {!cpu && !memory ? (
          <span className="small muted">{reason}</span>
        ) : (
          <>
            {cpu && <AxisBar title="Processeur" axis={cpu} fit={fit} format={millicores} />}
            {memory && <AxisBar title="Mémoire" axis={memory} fit={fit} format={bytes} />}
            <span className="xs muted">
              La part occupée est la consommation mesurée du cluster ; la part ajoutée est ce que ce déploiement réserve. Les réservations déjà
              faites par les autres applications ne sont pas visibles dans cette mesure.
            </span>
          </>
        )}
        {/* Le stockage ne se compare à rien : le cluster n'annonce pas la place restante sur ses volumes. */}
        {footprint.storageBytes != null && <span className="small">Disque : {bytes(footprint.storageBytes)} demandés à la classe de stockage du cluster.</span>}
      </Section>

      {notes.length > 0 && (
        <Section title="À savoir">
          <ul className="deploy-notes">
            {notes.map((n, i) => (
              <NoteRow key={i} note={n} />
            ))}
          </ul>
        </Section>
      )}

      <Section
        title="Manifeste Kubernetes"
        actions={
          <>
            <span className="xs muted">Lecture seule</span>
            {render.data && (
              <button className="btn btn-ghost btn-sm" onClick={copy}>
                <Copy size={14} /> Copier
              </button>
            )}
            <button className="btn btn-sm" onClick={() => setShowYaml(!showYaml)} aria-expanded={showYaml}>
              {showYaml ? "Replier" : "Afficher"}
            </button>
          </>
        }
      >
        {yamlStatus}
        {showYaml && render.data && (
          <div className="deploy-yaml">
            <YamlEditor value={render.data} readOnly height="360px" />
          </div>
        )}
      </Section>
    </>
  );
}

/** Une ligne de la liste des objets créés. */
function ObjectRow({ kind, name, detail }: { kind: string; name: string; detail: string }) {
  return (
    <li>
      <Plus size={14} weight="bold" />
      <strong>{kind}</strong>
      <span className="mono selectable">{name}</span>
      <span className="muted">{detail}</span>
    </li>
  );
}

/** Une barre « occupé / ajouté / libre » avec ses chiffres. */
function AxisBar({ title, axis, fit, format }: { title: string; axis: Axis; fit: Fit; format: (v: number) => string }) {
  const before = beforeFraction(axis);
  const after = afterFraction(axis);
  const rest = remaining(axis);
  const pct = Math.round(afterRatio(axis) * 100);
  return (
    <div className={`deploy-axis deploy-fit-${fit}`}>
      <div className="row between">
        <strong className="small">{title}</strong>
        <span className="small muted">{rest >= 0 ? `${format(rest)} libres après` : `${format(-rest)} de trop`}</span>
      </div>
      <div
        className="deploy-axis-bar"
        role="img"
        aria-label={`${title} : ${format(axis.used)} occupés, ${format(axis.added)} réservés, sur ${format(axis.capacity)} ; ${pct} % après déploiement`}
      >
        {/* La part ajoutée est peinte d'abord, la part occupée par-dessus : les deux se lisent de gauche à droite. */}
        <span className="added" style={{ width: `${after * 100}%` }} />
        <span className="used" style={{ width: `${before * 100}%` }} />
      </div>
      <div className="row wrap gap-12 xs deploy-legend">
        <span>
          <span className="sw used" /> {format(axis.used)} occupés
        </span>
        <span>
          <span className="sw added" /> + {format(axis.added)} réservés
        </span>
        <span>
          sur {format(axis.capacity)} — {pct} % après déploiement
        </span>
      </div>
    </div>
  );
}

function NoteRow({ note }: { note: Note }) {
  const IconCmp = NOTE_ICON[note.level];
  return (
    <li className={`deploy-note-${note.level}`}>
      <IconCmp size={15} weight="fill" />
      <span>{note.text}</span>
    </li>
  );
}

// ---------------------------------------------------------------------------
// Étape 4 — résultat
// ---------------------------------------------------------------------------

function StepResult({
  outcome,
  pending,
  error,
  dryRun,
  onView,
}: {
  outcome: { dryRun: boolean; value: ApplyOutcome } | null;
  pending: boolean;
  error: Error | null;
  dryRun: boolean;
  onView: (name: string, namespace: string | null) => void;
}) {
  if (pending || (!outcome && !error)) {
    return (
      <div className="deploy-intro">
        <h2>{dryRun ? "Simulation en cours…" : "Déploiement en cours…"}</h2>
        <Spinner label="Le serveur d'API traite les manifestes." />
      </div>
    );
  }
  if (!outcome) {
    return (
      <>
        <div className="deploy-intro">
          <h2 className="deploy-err">{dryRun ? "La simulation a échoué" : "Le déploiement a échoué"}</h2>
        </div>
        <Alert tone="err">{error?.message ?? "Erreur inconnue."}</Alert>
      </>
    );
  }

  const { value } = outcome;
  const ok = value.failed === 0;
  const title = outcome.dryRun ? (ok ? "La simulation est passée" : "La simulation a échoué") : ok ? "Application déployée" : "Le déploiement a échoué";
  const deployment = value.items.find((it) => it.resource.kind === "Deployment" && it.action !== "failed");

  return (
    <>
      <div className="deploy-intro">
        <h2 className={ok ? "deploy-ok" : "deploy-err"}>{title}</h2>
        {outcome.dryRun && ok && <p className="muted">Le serveur d'API a accepté les manifestes sans rien écrire. Vous pouvez déployer pour de bon.</p>}
      </div>
      <div className="panel">
        <div className="toolbar small">
          <strong className={ok ? "deploy-ok" : "deploy-err"}>
            {value.items.length} document(s) traité(s), {ok ? "aucun échec" : `${value.failed} en échec`}
          </strong>
          <span className="grow" />
          {!outcome.dryRun && deployment && (
            <button className="btn btn-sm" onClick={() => onView(deployment.resource.name, deployment.resource.namespace)}>
              <ArrowSquareOut size={14} /> Voir dans Ressources
            </button>
          )}
        </div>
        <table className="table">
          <thead>
            <tr>
              <th>Objet</th>
              <th>Nom</th>
              <th>Action</th>
              <th>Message</th>
            </tr>
          </thead>
          <tbody>
            {value.items.map((it, i) => (
              <tr key={i}>
                <td>{it.resource.kind}</td>
                <td className="mono selectable">
                  {it.resource.namespace ? `${it.resource.namespace}/` : ""}
                  {it.resource.name}
                </td>
                <td>
                  <Badge tone={ACTION_TONE[it.action]}>{ACTION_LABEL[it.action]}</Badge>
                </td>
                <td className="selectable" style={{ whiteSpace: "normal" }}>
                  {it.message ?? ""}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </>
  );
}
