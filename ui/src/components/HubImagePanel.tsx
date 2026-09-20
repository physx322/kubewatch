// Panneau latéral de l'onglet Images : tags de l'image sélectionnée, inspection
// du manifeste OCI du tag choisi et préremplissage de l'écran Déployer.
import { Package, Rocket, X } from "@phosphor-icons/react";
import { useQuery } from "@tanstack/react-query";
import { Fragment, useMemo, useState } from "react";
import { api } from "@/api/client";
import type { ImageDetails, TagInfo } from "@/api/hub";
import { Alert, Badge, Spinner } from "@/components/Basics";
import { bytes, dateTime } from "@/lib/format";
import {
  ExternalLink,
  openDeployDraft,
  parseEnv,
  repoUrl,
  sanitizeName,
  shortNameFromReference,
  sortTags,
  useHubStore,
  type TagSort,
} from "./HubCommon";

export function HubImagePanel({ image }: { image: string }) {
  const selectedTag = useHubStore((s) => s.selectedTag);
  const tagSort = useHubStore((s) => s.tagSort);
  const patch = useHubStore((s) => s.patch);
  const [tagFilter, setTagFilter] = useState("");

  const tags = useQuery({
    queryKey: ["hub", "tags", image],
    queryFn: () => api.hub.listTags(image),
    staleTime: 5 * 60_000,
  });

  const reference = selectedTag ? `${image}:${selectedTag}` : null;
  const details = useQuery({
    queryKey: ["hub", "inspect", reference],
    queryFn: () => api.hub.inspect(reference!),
    enabled: !!reference,
    staleTime: 10 * 60_000,
  });

  const visibleTags = useMemo(() => {
    const sorted = sortTags(tags.data ?? [], tagSort);
    const f = tagFilter.trim().toLowerCase();
    return f ? sorted.filter((t) => t.name.toLowerCase().includes(f) || (t.semver ?? "").includes(f)) : sorted;
  }, [tags.data, tagSort, tagFilter]);

  const close = () => patch({ selectedImage: null, selectedTag: null });

  // Les ports et variables viennent de l'inspection quand elle a abouti ; sinon
  // on transmet au moins la référence, à compléter dans l'écran Déployer.
  const deploy = () => {
    if (!reference) return;
    const d = details.data;
    openDeployDraft({
      image: d?.reference ?? reference,
      name: sanitizeName(shortNameFromReference(reference)),
      ports: d?.exposedPorts ?? [],
      env: d ? parseEnv(d.env) : [],
    });
  };

  return (
    <aside className="hub-side">
      <div className="toolbar">
        <Package size={16} />
        <strong className="truncate grow" title={image}>
          {image}
        </strong>
        <button className="btn btn-ghost btn-sm btn-icon" onClick={close} title="Fermer">
          <X size={14} />
        </button>
      </div>

      <div className="hub-side-body">
        <section className="hub-section">
          <div className="row">
            <span className="hub-section-title grow">
              Tags
              {tags.data && (
                <span className="muted">
                  {" "}
                  ({visibleTags.length}
                  {visibleTags.length !== tags.data.length ? ` / ${tags.data.length}` : ""})
                </span>
              )}
            </span>
            {tags.isFetching && <Spinner />}
          </div>
          <div className="row">
            <input
              className="input input-sm mono grow"
              placeholder="Filtrer les tags"
              value={tagFilter}
              onChange={(e) => setTagFilter(e.target.value)}
            />
            <select
              className="select select-sm"
              value={tagSort}
              onChange={(e) => patch({ tagSort: e.target.value as TagSort })}
              title="Ordre de tri"
            >
              <option value="version">Par version</option>
              <option value="date">Par date</option>
              <option value="name">Par nom</option>
            </select>
          </div>
          {tags.error && <Alert tone="err">{tags.error.message}</Alert>}
          {tags.isLoading && <Spinner label="Chargement des tags…" />}
          {tags.data && tags.data.length === 0 && (
            <div className="muted small">Aucun tag publié n'a été retourné par le registre.</div>
          )}
          {tags.data && tags.data.length > 0 && visibleTags.length === 0 && (
            <div className="muted small">Aucun tag ne correspond au filtre.</div>
          )}
          {visibleTags.length > 0 && (
            <div className="hub-tags">
              {visibleTags.map((t) => (
                <TagRow key={t.name} tag={t} active={t.name === selectedTag} onSelect={() => patch({ selectedTag: t.name })} />
              ))}
            </div>
          )}
        </section>

        <div className="divider" />

        <section className="hub-section">
          <div className="row">
            <span className="hub-section-title grow">Inspection</span>
            {details.isFetching && <Spinner />}
          </div>
          {!reference && <div className="muted small">Sélectionnez un tag pour inspecter l'image.</div>}
          {details.isLoading && <Spinner label="Inspection du manifeste OCI…" />}
          {details.error && <Alert tone="err">{details.error.message}</Alert>}
          {details.data && <DetailsView d={details.data} />}
        </section>
      </div>

      <div className="hub-side-footer">
        <button
          className="btn btn-primary"
          disabled={!reference}
          onClick={deploy}
          title={reference ? `Préremplir l'écran Déployer avec ${reference}` : "Choisissez d'abord un tag"}
        >
          <Rocket size={14} /> Déployer cette image
        </button>
        {reference && (
          <span className="muted xs mono truncate" title={reference}>
            {reference}
          </span>
        )}
      </div>
    </aside>
  );
}

function TagRow({ tag, active, onSelect }: { tag: TagInfo; active: boolean; onSelect: () => void }) {
  const detail: string[] = [];
  if (tag.sizeBytes != null) detail.push(bytes(tag.sizeBytes));
  if (tag.pushedAt) detail.push(dateTime(tag.pushedAt));
  if (tag.platforms.length > 0) detail.push(tag.platforms.join(", "));
  return (
    <button type="button" className={"hub-tag" + (active ? " active" : "")} onClick={onSelect} title={tag.digest ?? undefined}>
      <span className="row gap-4">
        <span className="mono hub-tag-name">{tag.name}</span>
        {tag.semver && tag.semver !== tag.name && <span className="muted xs">({tag.semver})</span>}
      </span>
      {detail.length > 0 && <span className="muted xs">{detail.join(" · ")}</span>}
    </button>
  );
}

function DetailsView({ d }: { d: ImageDetails }) {
  const platform = d.os && d.architecture ? `${d.os}/${d.architecture}` : (d.os ?? d.architecture ?? "—");
  const labels = Object.entries(d.labels);
  return (
    <div className="col gap-8">
      <div className="mono xs muted selectable truncate" title={d.reference}>
        {d.reference}
      </div>
      <div className="hub-kv">
        {d.digest && (
          <>
            <span className="k">Digest</span>
            <span className="v mono truncate" title={d.digest}>
              {d.digest}
            </span>
          </>
        )}
        <span className="k">Plateforme</span>
        <span className="v">{platform}</span>
        <span className="k">Ports exposés</span>
        <span className="v">
          {d.exposedPorts.length > 0
            ? d.exposedPorts.map((p) => (
                <span key={p} className="tag">
                  {p}
                </span>
              ))
            : "aucun"}
        </span>
        {d.entrypoint.length > 0 && (
          <>
            <span className="k">Entrypoint</span>
            <span className="v mono">{d.entrypoint.join(" ")}</span>
          </>
        )}
        {d.cmd.length > 0 && (
          <>
            <span className="k">Commande</span>
            <span className="v mono">{d.cmd.join(" ")}</span>
          </>
        )}
        {d.sourceRepo && (
          <>
            <span className="k">Dépôt source</span>
            <span className="v truncate">
              <ExternalLink href={repoUrl(d.sourceRepo)}>{d.sourceRepo}</ExternalLink>
            </span>
          </>
        )}
      </div>

      {d.env.length > 0 && (
        <details className="hub-details">
          <summary>
            Variables d'environnement <Badge>{d.env.length}</Badge>
          </summary>
          <div className="hub-details-body col gap-4">
            {d.env.map((e, i) => (
              <span key={i} className="mono xs selectable hub-wrap">
                {e}
              </span>
            ))}
          </div>
        </details>
      )}

      {labels.length > 0 && (
        <details className="hub-details">
          <summary>
            Étiquettes <Badge>{labels.length}</Badge>
          </summary>
          <div className="hub-details-body hub-kv">
            {labels.map(([k, v]) => (
              <Fragment key={k}>
                <span className="k mono">{k}</span>
                <span className="v xs truncate" title={v}>
                  {v}
                </span>
              </Fragment>
            ))}
          </div>
        </details>
      )}
    </div>
  );
}
