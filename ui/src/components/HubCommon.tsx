// ---------------------------------------------------------------------------
// Éléments partagés de l'écran Hub : état conservé entre deux passages sur
// l'écran, brouillon transmis à l'écran Déployer,
// libellés des registres et petites mises en forme.
// ---------------------------------------------------------------------------
import type { MouseEvent, ReactNode } from "react";
import { create } from "zustand";
import { api, describeError } from "@/api/client";
import type { RegistryKind, TagInfo } from "@/api/hub";
import { notify, useStore } from "@/app/store";

export type HubTab = "images" | "charts" | "catalog";
export type TagSort = "version" | "date" | "name";

interface HubState {
  tab: HubTab;
  imageQuery: string;
  registry: RegistryKind;
  /** Dernière recherche d'images lancée (la saisie peut avoir changé depuis). */
  imageSearch: { query: string; registry: RegistryKind } | null;
  selectedImage: string | null;
  selectedTag: string | null;
  tagSort: TagSort;
  chartQuery: string;
  /** Dernière recherche de charts lancée. */
  chartSearch: string | null;
  patch: (p: Partial<Omit<HubState, "patch">>) => void;
}

/** État du Hub, hors React Query : survit au changement d'écran, pas au redémarrage. */
export const useHubStore = create<HubState>()((set) => ({
  tab: "images",
  imageQuery: "",
  registry: "dockerHub",
  imageSearch: null,
  selectedImage: null,
  selectedTag: null,
  tagSort: "version",
  chartQuery: "",
  chartSearch: null,
  patch: (p) => set(p),
}));

// --- Registres -------------------------------------------------------------

export const REGISTRIES: { value: RegistryKind; label: string; placeholder: string; keyword: boolean }[] = [
  { value: "dockerHub", label: "Docker Hub", placeholder: "nginx, postgres, redis…", keyword: true },
  { value: "ghcr", label: "GitHub Container Registry", placeholder: "propriétaire/image", keyword: false },
  { value: "quay", label: "Quay", placeholder: "prometheus, node-exporter…", keyword: true },
  { value: "generic", label: "Registre OCI", placeholder: "registre.local:5000/equipe/app", keyword: false },
];

export function registryLabel(kind: RegistryKind): string {
  return REGISTRIES.find((r) => r.value === kind)?.label ?? kind;
}

// --- Brouillon de déploiement ---------------------------------------------

export const DEPLOY_DRAFT_KEY = "kubewatch.deployDraft";

export interface DeployDraft {
  image: string;
  name?: string;
  ports?: number[];
  env?: [string, string][];
  catalogAppId?: string;
}

/** Enregistre le brouillon et bascule sur l'écran Déployer, qui le lit au montage. */
export function openDeployDraft(draft: DeployDraft): void {
  try {
    sessionStorage.setItem(DEPLOY_DRAFT_KEY, JSON.stringify(draft));
  } catch (e) {
    notify.err("enregistrement du brouillon de déploiement impossible : " + describeError(e));
    return;
  }
  useStore.getState().setView("deploy");
}

/** `ghcr.io/o/depot:1.2` → `depot`. */
export function shortNameFromReference(reference: string): string {
  const noDigest = reference.split("@")[0] ?? reference;
  const last = noDigest.split("/").pop() ?? noDigest;
  return last.split(":")[0] ?? last;
}

/** Ramène un texte quelconque sur une étiquette DNS-1123 utilisable comme nom. */
export function sanitizeName(raw: string): string {
  let out = "";
  for (const c of raw.trim().toLowerCase()) {
    if (/^[a-z0-9]$/.test(c)) out += c;
    else if (!out.endsWith("-")) out += "-";
  }
  out = out.replace(/^-+|-+$/g, "").slice(0, 63).replace(/-+$/g, "");
  return out || "application";
}

/** `CLÉ=valeur` → paires ; les entrées sans clé sont ignorées. */
export function parseEnv(env: string[]): [string, string][] {
  const out: [string, string][] = [];
  for (const e of env) {
    const i = e.indexOf("=");
    if (i <= 0) continue;
    out.push([e.slice(0, i), e.slice(i + 1)]);
  }
  return out;
}

// --- Mise en forme ---------------------------------------------------------

/** Grand nombre abrégé : `1,2 M`. */
export function shortNumber(n: number | null | undefined): string {
  if (n == null || Number.isNaN(n)) return "–";
  const a = Math.abs(n);
  const fmt = (v: number, u: string) => `${v.toLocaleString("fr-FR", { maximumFractionDigits: 1 })} ${u}`;
  if (a >= 1e9) return fmt(n / 1e9, "G");
  if (a >= 1e6) return fmt(n / 1e6, "M");
  if (a >= 1e3) return fmt(n / 1e3, "k");
  return n.toLocaleString("fr-FR");
}

/** URL du dépôt source : une URL complète est gardée, un `propriétaire/dépôt` va sur GitHub. */
export function repoUrl(raw: string): string {
  const r = raw.trim();
  if (/^https?:\/\//i.test(r)) return r;
  return "https://github.com/" + r.replace(/^\/+/, "");
}

/** Lien ouvert dans le navigateur du système via le backend. */
export function ExternalLink({
  href,
  children,
  className,
  title,
}: {
  href: string;
  children: ReactNode;
  className?: string;
  title?: string;
}) {
  const open = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    api.openUrl(href).catch((err: Error) => notify.err(err.message));
  };
  return (
    <a href={href} onClick={open} className={className} title={title ?? href}>
      {children}
    </a>
  );
}

// --- Tri des tags ----------------------------------------------------------

function parseSemver(s: string): { nums: number[]; pre: string[] | null } {
  const noBuild = s.split("+")[0] ?? s;
  const dash = noBuild.indexOf("-");
  const core = dash >= 0 ? noBuild.slice(0, dash) : noBuild;
  const pre = dash >= 0 ? noBuild.slice(dash + 1).split(".") : null;
  const nums = core.split(".").map((p) => Number.parseInt(p, 10) || 0);
  return { nums, pre };
}

/** Ordre décroissant de versions sémantiques (`semver` normalisé par le backend). */
export function compareSemverDesc(a: string, b: string): number {
  const x = parseSemver(a);
  const y = parseSemver(b);
  for (let i = 0; i < 3; i++) {
    const d = (y.nums[i] ?? 0) - (x.nums[i] ?? 0);
    if (d !== 0) return d;
  }
  // Une version finale passe avant ses pré-versions.
  if (!x.pre && y.pre) return -1;
  if (x.pre && !y.pre) return 1;
  if (x.pre && y.pre) {
    const n = Math.max(x.pre.length, y.pre.length);
    for (let i = 0; i < n; i++) {
      const p = x.pre[i];
      const q = y.pre[i];
      if (p === undefined) return 1;
      if (q === undefined) return -1;
      const pn = /^\d+$/.test(p);
      const qn = /^\d+$/.test(q);
      if (pn && qn) {
        const d = Number(q) - Number(p);
        if (d !== 0) return d;
      } else if (pn !== qn) {
        return pn ? 1 : -1;
      } else {
        const d = q.localeCompare(p);
        if (d !== 0) return d;
      }
    }
  }
  return 0;
}

/**
 * Trie une copie de la liste. « version » reproduit l'ordre du backend pour
 * l'API Registry v2 : versions décroissantes, puis les tags sans version.
 */
export function sortTags(tags: TagInfo[], mode: TagSort): TagInfo[] {
  const arr = tags.slice();
  if (mode === "version") {
    arr.sort((a, b) => {
      if (a.semver && b.semver) return compareSemverDesc(a.semver, b.semver) || b.name.localeCompare(a.name);
      if (a.semver) return -1;
      if (b.semver) return 1;
      return b.name.localeCompare(a.name);
    });
  } else if (mode === "date") {
    const t = (iso: string | null) => (iso ? Date.parse(iso) || 0 : 0);
    arr.sort((a, b) => t(b.pushedAt) - t(a.pushedAt));
  } else {
    arr.sort((a, b) => a.name.localeCompare(b.name, "fr", { numeric: true }));
  }
  return arr;
}
