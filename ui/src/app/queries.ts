// Hooks de données partagés (React Query) et invalidations.
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { api } from "@/api/client";
import { useStore } from "./store";

export const REFRESH_MS = 5_000;

export function useClusters() {
  return useQuery({
    queryKey: ["clusters"],
    queryFn: api.clusters.list,
    refetchInterval: 30_000,
  });
}

export function useNamespaces(cluster: string | null) {
  return useQuery({
    queryKey: ["namespaces", cluster],
    queryFn: () => api.resources.namespaces(cluster!),
    enabled: !!cluster,
    staleTime: 60_000,
  });
}

export function useKinds(cluster: string | null) {
  return useQuery({
    queryKey: ["kinds", cluster],
    queryFn: () => api.resources.kinds(cluster!),
    enabled: !!cluster,
    staleTime: 5 * 60_000,
  });
}

export function useOverview(cluster: string | null) {
  const auto = useStore((s) => s.autoRefresh);
  return useQuery({
    queryKey: ["overview", cluster],
    queryFn: () => api.resources.overview(cluster!),
    enabled: !!cluster,
    refetchInterval: auto ? 10_000 : false,
  });
}

export function useEvents(cluster: string | null, namespace: string | null) {
  const auto = useStore((s) => s.autoRefresh);
  return useQuery({
    queryKey: ["events", cluster, namespace],
    queryFn: () => api.resources.events(cluster!, namespace),
    enabled: !!cluster,
    refetchInterval: auto ? 15_000 : false,
  });
}

export function useAiSettings() {
  return useQuery({ queryKey: ["ai-settings"], queryFn: api.ai.settings, staleTime: Infinity });
}

/**
 * Au démarrage : avertissements du backend, choix d'un cluster par défaut et
 * rafraîchissement de la liste quand le backend signale un changement.
 */
export function useBackendLifecycle() {
  const qc = useQueryClient();
  const toast = useStore((s) => s.toast);

  useEffect(() => {
    let cancelled = false;
    api
      .appInfo()
      .then((info) => {
        if (cancelled) return;
        for (const w of info.warnings) toast("warn", w);
      })
      .catch(() => {});

    const unlistenChanged = api.onClustersChanged(async () => {
      await qc.invalidateQueries({ queryKey: ["clusters"] });
      const st = useStore.getState();
      const clusters = await api.clusters.list().catch(() => []);
      const known = clusters.find((c) => c.name === st.cluster);
      if (!known) {
        const current = await api.clusters.current().catch(() => null);
        const fallback = current ?? clusters.find((c) => c.connected)?.name ?? clusters[0]?.name ?? null;
        st.setCluster(fallback);
      }
    });
    const unlistenWarn = api.onStartupWarning((m) => toast("warn", m));

    return () => {
      cancelled = true;
      unlistenChanged.then((f) => f()).catch(() => {});
      unlistenWarn.then((f) => f()).catch(() => {});
    };
  }, [qc, toast]);
}
