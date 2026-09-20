// Types de ressources : favoris, colonnes supplémentaires, références.
import type { ObjectSummary, ResourceKind, ResourceRef } from "@/api/types";

/** Types proposés en tête de liste, dans cet ordre. */
export const FAVOURITE_KINDS = [
  "pods",
  "deployments",
  "statefulsets",
  "daemonsets",
  "replicasets",
  "jobs",
  "cronjobs",
  "services",
  "ingresses",
  "configmaps",
  "secrets",
  "persistentvolumeclaims",
  "persistentvolumes",
  "nodes",
  "namespaces",
  "events",
  "serviceaccounts",
  "horizontalpodautoscalers",
];

export const WORKLOAD_KINDS = new Set(["Deployment", "StatefulSet", "DaemonSet", "ReplicaSet"]);
export const SCALABLE_KINDS = new Set(["Deployment", "StatefulSet", "ReplicaSet"]);
export const ROLLBACK_KINDS = new Set(["Deployment", "StatefulSet", "DaemonSet"]);

/** Colonnes tirées de `extra`, par pluriel (clés telles que produites par le cœur). */
export const EXTRA_COLUMNS: Record<string, { key: string; label: string; num?: boolean }[]> = {
  pods: [
    { key: "podIP", label: "IP" },
    { key: "qosClass", label: "QoS" },
  ],
  deployments: [
    { key: "desired", label: "Voulues", num: true },
    { key: "upToDate", label: "À jour", num: true },
    { key: "available", label: "Dispo.", num: true },
    { key: "strategy", label: "Stratégie" },
  ],
  statefulsets: [
    { key: "desired", label: "Voulues", num: true },
    { key: "upToDate", label: "À jour", num: true },
    { key: "available", label: "Dispo.", num: true },
  ],
  replicasets: [
    { key: "desired", label: "Voulues", num: true },
    { key: "current", label: "Actuelles", num: true },
    { key: "available", label: "Dispo.", num: true },
  ],
  daemonsets: [
    { key: "desired", label: "Voulus", num: true },
    { key: "current", label: "Actuels", num: true },
    { key: "upToDate", label: "À jour", num: true },
    { key: "available", label: "Dispo.", num: true },
    { key: "misscheduled", label: "Mal placés", num: true },
  ],
  jobs: [
    { key: "completions", label: "Complétions" },
    { key: "active", label: "Actifs", num: true },
    { key: "failed", label: "Échecs", num: true },
    { key: "duration", label: "Durée" },
  ],
  cronjobs: [
    { key: "schedule", label: "Planification" },
    { key: "suspend", label: "Suspendu" },
    { key: "active", label: "Actifs", num: true },
    { key: "lastSchedule", label: "Dernière exécution" },
  ],
  services: [
    { key: "type", label: "Type" },
    { key: "clusterIP", label: "IP cluster" },
    { key: "externalIP", label: "IP externe" },
    { key: "ports", label: "Ports" },
  ],
  ingresses: [
    { key: "class", label: "Classe" },
    { key: "hosts", label: "Hôtes" },
    { key: "address", label: "Adresse" },
    { key: "tls", label: "TLS" },
  ],
  nodes: [
    { key: "roles", label: "Rôles" },
    { key: "version", label: "Version" },
    { key: "internalIP", label: "IP interne" },
    { key: "os", label: "OS" },
    { key: "containerRuntime", label: "Runtime" },
    { key: "schedulable", label: "Ordonnançable" },
  ],
  persistentvolumeclaims: [
    { key: "capacity", label: "Capacité" },
    { key: "storageClass", label: "Classe" },
    { key: "accessModes", label: "Accès" },
    { key: "volume", label: "Volume" },
  ],
  persistentvolumes: [
    { key: "capacity", label: "Capacité" },
    { key: "storageClass", label: "Classe" },
    { key: "accessModes", label: "Accès" },
    { key: "claim", label: "Réclamation" },
  ],
  configmaps: [{ key: "keys", label: "Clés", num: true }],
  secrets: [
    { key: "type", label: "Type" },
    { key: "keys", label: "Clés", num: true },
  ],
};

export function extraText(v: unknown): string {
  if (v == null) return "";
  if (Array.isArray(v)) return v.map(extraText).join(", ");
  if (typeof v === "object") return JSON.stringify(v);
  return String(v);
}

export function splitApiVersion(apiVersion: string): { group: string; version: string } {
  const i = apiVersion.indexOf("/");
  return i < 0 ? { group: "", version: apiVersion } : { group: apiVersion.slice(0, i), version: apiVersion.slice(i + 1) };
}

/** Référence complète d'un objet, à l'aide du catalogue des types. */
export function refOf(obj: ObjectSummary, kinds: ResourceKind[] | undefined): ResourceRef {
  const { group, version } = splitApiVersion(obj.apiVersion);
  const kind =
    kinds?.find((k) => k.kind === obj.kind && k.group === group) ?? kinds?.find((k) => k.kind === obj.kind);
  return {
    group: kind?.group ?? group,
    version: kind?.version ?? version,
    kind: obj.kind,
    plural: kind?.plural ?? obj.kind.toLowerCase() + "s",
    namespace: obj.namespace,
    name: obj.name,
  };
}

export function objectKey(obj: ObjectSummary): string {
  return `${obj.kind}/${obj.namespace ?? ""}/${obj.name}`;
}

export function pluralOf(kind: string, kinds: ResourceKind[] | undefined): string {
  return kinds?.find((k) => k.kind === kind)?.plural ?? kind.toLowerCase() + "s";
}

export function kindLabel(k: ResourceKind): string {
  return k.group ? `${k.plural}.${k.group}` : k.plural;
}
