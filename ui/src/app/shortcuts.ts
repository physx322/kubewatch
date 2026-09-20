// Raccourcis clavier globaux.
import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { useStore, type View } from "./store";

const VIEW_KEYS: Record<string, View> = {
  "1": "overview",
  "2": "resources",
  "3": "topology",
  "4": "hub",
  "5": "deploy",
  "6": "updates",
  "7": "clusters",
  ",": "settings",
};

export function useShortcuts() {
  const qc = useQueryClient();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
      const st = useStore.getState();
      const view = VIEW_KEYS[e.key];
      if (view && !e.shiftKey) {
        e.preventDefault();
        st.setView(view);
        return;
      }
      if (e.key.toLowerCase() === "j") {
        e.preventDefault();
        st.toggleAssistant();
      } else if (e.key.toLowerCase() === "k") {
        e.preventDefault();
        st.focusSearch();
      } else if (e.key.toLowerCase() === "r") {
        // Rafraîchit toutes les lectures de l'écran courant.
        e.preventDefault();
        void qc.invalidateQueries();
        st.toast("info", "Données rafraîchies.");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [qc]);
}
