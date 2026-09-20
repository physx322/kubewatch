import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath, URL } from "node:url";

// Hôte Tauri : la variable est posée par `cargo tauri dev` / `cargo tauri build`.
const host = process.env.TAURI_DEV_HOST;
const platform = process.env.TAURI_ENV_PLATFORM;

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
  },
  // Tauri affiche déjà ses propres messages : ne pas effacer la console.
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 5174 } : undefined,
    watch: { ignored: ["**/crates/**", "**/target/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    // WebKitGTK sous Linux, WebView2 (Chromium) sous Windows.
    target: platform === "windows" ? "chrome105" : "safari13",
    minify: process.env.TAURI_ENV_DEBUG ? false : "esbuild",
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    chunkSizeWarningLimit: 900,
    rollupOptions: {
      output: {
        // Les grosses bibliothèques dans leurs propres morceaux : l'écran
        // d'accueil n'a pas à analyser l'éditeur, le terminal ni le graphe.
        manualChunks: {
          "vendor-react": ["react", "react-dom", "zustand", "@tanstack/react-query", "@tanstack/react-table"],
          "vendor-editor": ["@uiw/react-codemirror", "@codemirror/lang-yaml", "@codemirror/language", "@codemirror/state", "@codemirror/view"],
          "vendor-terminal": ["@xterm/xterm", "@xterm/addon-fit"],
          "vendor-markdown": ["react-markdown", "remark-gfm"],
          "vendor-graph": ["d3-force"],
          "vendor-icons": ["@phosphor-icons/react"],
        },
      },
    },
  },
});
