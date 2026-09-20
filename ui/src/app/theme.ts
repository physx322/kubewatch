// Thème : application de data-theme sur <html> et détection du mode sombre.
import { useEffect, useSyncExternalStore } from "react";
import { useStore } from "./store";

const mq = window.matchMedia("(prefers-color-scheme: dark)");

export function useApplyTheme() {
  const theme = useStore((s) => s.theme);
  useEffect(() => {
    const root = document.documentElement;
    if (theme === "auto") root.removeAttribute("data-theme");
    else root.setAttribute("data-theme", theme);
  }, [theme]);
}

function subscribe(cb: () => void) {
  mq.addEventListener("change", cb);
  const unsub = useStore.subscribe(cb);
  return () => {
    mq.removeEventListener("change", cb);
    unsub();
  };
}

function isDark(): boolean {
  const theme = useStore.getState().theme;
  if (theme === "dark") return true;
  if (theme === "light") return false;
  return mq.matches;
}

export function useDarkMode(): boolean {
  return useSyncExternalStore(subscribe, isDark);
}
