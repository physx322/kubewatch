// Écran Hub : recherche d'images de conteneurs, charts Helm (Artifact Hub) et
// catalogue d'applications prêtes à déployer. Le formulaire de déploiement de
// le formulaire de déploiement est un écran à part entière : le Hub lui transmet un
// brouillon (image, nom, ports, variables) et bascule dessus.
import {
  ArrowSquareOut,
  ArrowsClockwise,
  BookOpen,
  DownloadSimple,
  HardDrive,
  MagnifyingGlass,
  Package,
  Rocket,
  Stack,
  Star,
  Storefront,
  type Icon,
} from "@phosphor-icons/react";
import { useQuery } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";
import { useMemo, useState, type FormEvent } from "react";
import { api } from "@/api/client";
import type { CatalogApp, ChartSummary, ImageSummary, RegistryKind } from "@/api/hub";
import { useStore } from "@/app/store";
import { Alert, Badge, EmptyState, Spinner } from "@/components/Basics";
import { DataTable } from "@/components/DataTable";
import {
  ExternalLink,
  openDeployDraft,
  REGISTRIES,
  registryLabel,
  sanitizeName,
  shortNumber,
  useHubStore,
  type HubTab,
} from "@/components/HubCommon";
import { HubImagePanel } from "@/components/HubImagePanel";
import { count, dateTime, relativeTime } from "@/lib/format";
import "./Hub.css";

const TABS: { id: HubTab; label: string; icon: Icon }[] = [
  { id: "images", label: "Images", icon: Package },
  { id: "charts", label: "Charts Helm", icon: Stack },
  { id: "catalog", label: "Catalogue", icon: Storefront },
];

export function HubView() {
  const tab = useHubStore((s) => s.tab);
  const patch = useHubStore((s) => s.patch);
  const setView = useStore((s) => s.setView);
  return (
    <div className="fill hub">
      <div className="tabs hub-tabs">
        {TABS.map((t) => (
          <button key={t.id} className={"tab" + (tab === t.id ? " active" : "")} onClick={() => patch({ tab: t.id })}>
            <t.icon size={14} />
            {t.label}
          </button>
        ))}
        <span className="grow" />
        <button className="btn btn-ghost btn-sm" onClick={() => setView("deploy")} title="Ouvrir l'écran Déployer">
          <Rocket size={14} /> Écran Déployer
        </button>
      </div>
      {tab === "images" && <ImagesTab />}
      {tab === "charts" && <ChartsTab />}
      {tab === "catalog" && <CatalogTab />}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Onglet « Images »
// ---------------------------------------------------------------------------

function ImagesTab() {
  const filter = useStore((s) => s.filter);
  const imageQuery = useHubStore((s) => s.imageQuery);
  const registry = useHubStore((s) => s.registry);
  const imageSearch = useHubStore((s) => s.imageSearch);
  const selectedImage = useHubStore((s) => s.selectedImage);
  const patch = useHubStore((s) => s.patch);
  const [message, setMessage] = useState<string | null>(null);

  const search = useQuery({
    queryKey: ["hub", "images", imageSearch?.registry, imageSearch?.query],
    queryFn: () => api.hub.searchImages(imageSearch!.query, imageSearch!.registry),
    enabled: !!imageSearch,
    staleTime: 5 * 60_000,
  });

  const launch = (e?: FormEvent) => {
    e?.preventDefault();
    const query = imageQuery.trim();
    if (!query) {
      setMessage("Indiquez un terme de recherche.");
      return;
    }
    setMessage(null);
    if (imageSearch && imageSearch.query === query && imageSearch.registry === registry) {
      search.refetch();
    } else {
      patch({ imageSearch: { query, registry }, selectedImage: null, selectedTag: null });
    }
  };

  const select = (img: ImageSummary) =>
    patch({ selectedImage: selectedImage === img.name ? null : img.name, selectedTag: null });

  const columns = useMemo<ColumnDef<ImageSummary, unknown>[]>(
    () => [
      {
        id: "name",
        header: "Image",
        accessorKey: "name",
        cell: (c) => {
          const o = c.row.original;
          return (
            <span className="row gap-4">
              <strong>{o.name}</strong>
              {o.official && <Badge tone="ok">officielle</Badge>}
            </span>
          );
        },
      },
      {
        id: "description",
        header: "Description",
        accessorFn: (o) => o.description ?? "",
        cell: (c) => {
          const d = c.getValue<string>();
          return d ? (
            <span className="hub-desc muted" title={d}>
              {d}
            </span>
          ) : (
            <span className="faint">–</span>
          );
        },
      },
      {
        id: "stars",
        header: "Étoiles",
        accessorFn: (o) => o.stars ?? 0,
        meta: { num: true },
        cell: (c) =>
          c.row.original.stars == null ? (
            <span className="faint">–</span>
          ) : (
            <span className="hub-num">
              <Star size={12} />
              {shortNumber(c.row.original.stars)}
            </span>
          ),
      },
      {
        id: "pulls",
        header: "Téléchargements",
        accessorFn: (o) => o.pulls ?? 0,
        meta: { num: true },
        cell: (c) =>
          c.row.original.pulls == null ? (
            <span className="faint">–</span>
          ) : (
            <span className="hub-num" title={c.row.original.pulls.toLocaleString("fr-FR")}>
              <DownloadSimple size={12} />
              {shortNumber(c.row.original.pulls)}
            </span>
          ),
      },
      {
        id: "updated",
        header: "Mise à jour",
        accessorFn: (o) => (o.updatedAt ? Date.parse(o.updatedAt) || 0 : 0),
        meta: { num: true },
        cell: (c) =>
          c.row.original.updatedAt ? (
            <span className="muted" title={dateTime(c.row.original.updatedAt)}>
              {relativeTime(c.row.original.updatedAt)}
            </span>
          ) : (
            <span className="faint">–</span>
          ),
      },
      {
        id: "registry",
        header: "Registre",
        accessorFn: (o) => registryLabel(o.registry),
        cell: (c) => <span className="muted small">{c.getValue<string>()}</span>,
      },
    ],
    [],
  );

  const reg = REGISTRIES.find((r) => r.value === registry) ?? REGISTRIES[0]!;

  return (
    <div className="fill" style={{ flexDirection: "row" }}>
      <div className="fill" style={{ borderRight: selectedImage ? "1px solid var(--border)" : undefined }}>
        <form className="toolbar" onSubmit={launch}>
          <div className="search hub-search">
            <MagnifyingGlass size={14} />
            <input
              className="input input-sm"
              placeholder={reg.placeholder}
              value={imageQuery}
              onChange={(e) => patch({ imageQuery: e.target.value })}
              title={reg.keyword ? "Recherche par mot-clé" : "Référence complète de l'image (pas de recherche par mot-clé)"}
            />
          </div>
          <select
            className="select select-sm"
            value={registry}
            onChange={(e) => patch({ registry: e.target.value as RegistryKind })}
            title="Registre interrogé"
          >
            {REGISTRIES.map((r) => (
              <option key={r.value} value={r.value}>
                {r.label}
              </option>
            ))}
          </select>
          <button type="submit" className="btn btn-primary btn-sm" disabled={search.isFetching}>
            Rechercher
          </button>
          {search.isFetching && <Spinner />}
          <span className="muted small">{search.data ? count(search.data.length, "image") : ""}</span>
          <span className="grow" />
          {imageSearch && (
            <button type="button" className="btn btn-sm btn-icon" onClick={() => search.refetch()} title="Actualiser">
              <ArrowsClockwise size={14} />
            </button>
          )}
        </form>

        {message && (
          <div className="hub-notice">
            <Alert tone="warn">{message}</Alert>
          </div>
        )}
        {search.error && (
          <div className="hub-notice">
            <Alert tone="err">{search.error.message}</Alert>
          </div>
        )}

        {!imageSearch && (
          <EmptyState
            icon={MagnifyingGlass}
            title="Explorer un registre"
            hint={
              reg.keyword
                ? "Recherchez par mot-clé, puis choisissez une image pour parcourir ses tags et l'inspecter."
                : `${reg.label} ne propose pas de recherche par mot-clé : saisissez la référence complète de l'image, par exemple « ${reg.placeholder} ».`
            }
          />
        )}
        {search.data && (
          <DataTable
            data={search.data}
            columns={columns}
            filter={filter}
            rowKey={(o) => o.name}
            selectedKey={selectedImage}
            onRowClick={select}
            empty={
              <EmptyState
                icon={Package}
                title="Aucun résultat"
                hint={`Aucune image ne correspond à « ${imageSearch?.query ?? ""} »${filter ? " avec le filtre courant" : ""}.`}
              />
            }
          />
        )}
        {search.isLoading && (
          <div className="p-16">
            <Spinner label="Recherche…" />
          </div>
        )}
      </div>
      {selectedImage && (
        <div className="fill hub-side-col">
          <HubImagePanel key={selectedImage} image={selectedImage} />
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Onglet « Charts Helm »
// ---------------------------------------------------------------------------

function ChartsTab() {
  const filter = useStore((s) => s.filter);
  const chartQuery = useHubStore((s) => s.chartQuery);
  const chartSearch = useHubStore((s) => s.chartSearch);
  const patch = useHubStore((s) => s.patch);
  const [message, setMessage] = useState<string | null>(null);

  const search = useQuery({
    queryKey: ["hub", "charts", chartSearch],
    queryFn: () => api.hub.searchCharts(chartSearch!),
    enabled: !!chartSearch,
    staleTime: 5 * 60_000,
  });

  const launch = (e?: FormEvent) => {
    e?.preventDefault();
    const query = chartQuery.trim();
    if (!query) {
      setMessage("Indiquez un terme de recherche.");
      return;
    }
    setMessage(null);
    if (chartSearch === query) search.refetch();
    else patch({ chartSearch: query });
  };

  const rows = useMemo(() => {
    const f = filter.trim().toLowerCase();
    const all = search.data ?? [];
    if (!f) return all;
    return all.filter((c) =>
      [c.name, c.repository, c.version, c.appVersion ?? "", c.description ?? ""].join(" ").toLowerCase().includes(f),
    );
  }, [search.data, filter]);

  return (
    <div className="fill">
      <form className="toolbar" onSubmit={launch}>
        <div className="search hub-search">
          <MagnifyingGlass size={14} />
          <input
            className="input input-sm"
            placeholder="postgresql, ingress-nginx…"
            value={chartQuery}
            onChange={(e) => patch({ chartQuery: e.target.value })}
          />
        </div>
        <button type="submit" className="btn btn-primary btn-sm" disabled={search.isFetching}>
          Rechercher sur Artifact Hub
        </button>
        {search.isFetching && <Spinner />}
        <span className="muted small">
          {search.data ? `${count(rows.length, "chart")}${rows.length !== search.data.length ? ` sur ${search.data.length}` : ""}` : ""}
        </span>
        <span className="grow" />
        {chartSearch && (
          <button type="button" className="btn btn-sm btn-icon" onClick={() => search.refetch()} title="Actualiser">
            <ArrowsClockwise size={14} />
          </button>
        )}
      </form>

      {message && (
        <div className="hub-notice">
          <Alert tone="warn">{message}</Alert>
        </div>
      )}
      {search.error && (
        <div className="hub-notice">
          <Alert tone="err">{search.error.message}</Alert>
        </div>
      )}

      {!chartSearch && (
        <EmptyState icon={Stack} title="Aucun chart" hint="Lancez une recherche pour interroger Artifact Hub." />
      )}
      {search.isLoading && (
        <div className="p-16">
          <Spinner label="Recherche…" />
        </div>
      )}
      {search.data && rows.length === 0 && (
        <EmptyState
          icon={Stack}
          title="Aucun résultat"
          hint={`Aucun chart ne correspond à « ${chartSearch ?? ""} »${filter ? " avec le filtre courant" : ""}.`}
        />
      )}
      {rows.length > 0 && (
        <div className="scroll hub-list">
          {rows.map((c) => (
            <ChartCard key={`${c.repository}/${c.name}`} chart={c} />
          ))}
        </div>
      )}
    </div>
  );
}

function ChartCard({ chart }: { chart: ChartSummary }) {
  const hubUrl = `https://artifacthub.io/packages/helm/${encodeURIComponent(chart.repository)}/${encodeURIComponent(chart.name)}`;
  return (
    <div className="card hub-chart">
      <div className="row wrap gap-4">
        <strong>{chart.name}</strong>
        <span className="muted small">dépôt {chart.repository}</span>
        <Badge tone="info" title="Version du chart">
          chart {chart.version}
        </Badge>
        {chart.appVersion && <Badge title="Version de l'application empaquetée">application {chart.appVersion}</Badge>}
        {chart.stars != null && (
          <span className="muted small hub-num">
            <Star size={12} />
            {shortNumber(chart.stars)}
          </span>
        )}
      </div>
      {chart.description && (
        <div className="muted small hub-clamp" title={chart.description}>
          {chart.description}
        </div>
      )}
      <div className="row wrap gap-12 small">
        <ExternalLink href={hubUrl} className="hub-num">
          <ArrowSquareOut size={12} /> Artifact Hub
        </ExternalLink>
        {chart.home && <ExternalLink href={chart.home}>Site du projet</ExternalLink>}
        {chart.repoUrl && (
          <ExternalLink href={chart.repoUrl} className="mono xs muted" title="Dépôt Helm (helm repo add)">
            {chart.repoUrl}
          </ExternalLink>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Onglet « Catalogue »
// ---------------------------------------------------------------------------

function CatalogTab() {
  const filter = useStore((s) => s.filter);
  const catalog = useQuery({ queryKey: ["hub", "catalog"], queryFn: api.hub.catalog, staleTime: Infinity });

  // Regroupement par catégorie, ordre alphabétique stable.
  const groups = useMemo(() => {
    const f = filter.trim().toLowerCase();
    const apps = (catalog.data ?? []).filter(
      (a) => !f || [a.name, a.id, a.description, a.image, a.category].join(" ").toLowerCase().includes(f),
    );
    const map = new Map<string, CatalogApp[]>();
    for (const a of apps) {
      const arr = map.get(a.category) ?? [];
      arr.push(a);
      map.set(a.category, arr);
    }
    return [...map.entries()].sort((a, b) => a[0].localeCompare(b[0], "fr"));
  }, [catalog.data, filter]);

  return (
    <div className="fill">
      <div className="toolbar">
        <span className="muted small">Applications prêtes à déployer, embarquées dans KubeWatch.</span>
        {catalog.data && <span className="muted small">· {count(catalog.data.length, "application")}</span>}
        {catalog.isFetching && <Spinner />}
        <span className="grow" />
        <button className="btn btn-sm" onClick={() => catalog.refetch()}>
          <ArrowsClockwise size={14} /> Recharger
        </button>
      </div>

      {catalog.error && (
        <div className="hub-notice">
          <Alert tone="err">{catalog.error.message}</Alert>
        </div>
      )}
      {catalog.isLoading && (
        <div className="p-16">
          <Spinner label="Chargement du catalogue…" />
        </div>
      )}
      {catalog.data && groups.length === 0 && (
        <EmptyState
          icon={Storefront}
          title={catalog.data.length === 0 ? "Catalogue vide" : "Aucune application"}
          hint={catalog.data.length === 0 ? undefined : "Aucune application ne correspond au filtre courant."}
        />
      )}
      {groups.length > 0 && (
        <div className="scroll hub-catalog">
          {groups.map(([category, apps]) => (
            <section key={category} className="hub-category">
              <h3>
                {category} <span className="muted small">({apps.length})</span>
              </h3>
              <div className="hub-cards">
                {apps.map((a) => (
                  <CatalogCard key={a.id} app={a} />
                ))}
              </div>
            </section>
          ))}
        </div>
      )}
    </div>
  );
}

function CatalogCard({ app }: { app: CatalogApp }) {
  const deploy = () =>
    openDeployDraft({
      image: app.image,
      name: sanitizeName(app.id),
      ports: app.defaultPort != null ? [app.defaultPort] : [],
      env: app.env,
      catalogAppId: app.id,
    });
  return (
    <div className="card hub-card">
      <div className="hub-card-body">
        <div className="row">
          <strong className="grow truncate" title={app.name}>
            {app.name}
          </strong>
          {app.needsPvc && (
            <Badge tone="warn" title="Cette application a besoin d'un volume persistant">
              <HardDrive size={11} /> volume
            </Badge>
          )}
        </div>
        <div className="muted small hub-clamp" title={app.description}>
          {app.description}
        </div>
        <div className="mono xs muted truncate selectable" title={app.image}>
          {app.image}
        </div>
        <div className="row wrap gap-4">
          {app.defaultPort != null && <span className="tag">port {app.defaultPort}</span>}
          {app.env.length > 0 && (
            <span className="tag" title={app.env.map(([k, v]) => `${k}=${v}`).join("\n")}>
              {count(app.env.length, "variable")}
            </span>
          )}
          {app.chart && (
            <span className="tag" title={`Chart Helm équivalent : ${app.chart.repository}/${app.chart.name} ${app.chart.version}`}>
              chart {app.chart.repository}/{app.chart.name}
            </span>
          )}
        </div>
      </div>
      <div className="hub-card-footer">
        {app.docsUrl ? (
          <ExternalLink href={app.docsUrl} className="small hub-num">
            <BookOpen size={12} /> Documentation
          </ExternalLink>
        ) : (
          <span />
        )}
        <button className="btn btn-primary btn-sm" onClick={deploy}>
          <Rocket size={12} /> Déployer
        </button>
      </div>
    </div>
  );
}
