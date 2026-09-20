// Test de fumée des écrans : `npm test` (voir screens.tsx).
// 1. esbuild regroupe screens.tsx avec les faux modules Tauri ;
// 2. jsdom fournit un DOM ; 3. chaque écran est monté avec la fixture.
import { build } from "esbuild";
import { JSDOM } from "jsdom";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const ui = path.resolve(here, "../..");
const out = path.join(here, ".bundle.cjs");

await build({
  entryPoints: [path.join(here, "screens.tsx")],
  bundle: true,
  format: "cjs",
  platform: "node",
  target: "es2022",
  outfile: out,
  jsx: "automatic",
  loader: { ".css": "empty" },
  alias: {
    "@": path.join(ui, "src"),
    "@tauri-apps/api/core": path.join(here, "tauri-core.mock.ts"),
    "@tauri-apps/api/event": path.join(here, "tauri-event.mock.ts"),
  },
  define: { "process.env.NODE_ENV": '"development"' },
  logLevel: "warning",
});

const dom = new JSDOM('<!doctype html><html><body><div id="root"></div></body></html>', { pretendToBeVisual: true, url: "http://localhost/" });
const w = dom.window;
// Node expose déjà certains globaux (navigator, localStorage…) en lecture seule :
// on les redéfinit explicitement pour pointer vers ceux de jsdom.
const def = (k, v) => Object.defineProperty(globalThis, k, { value: v, configurable: true, writable: true });
def("window", w);
def("self", w);
def("document", w.document);
def("navigator", w.navigator);
def("localStorage", w.localStorage);
def("sessionStorage", w.sessionStorage);
for (const k of ["HTMLElement", "HTMLInputElement", "HTMLTextAreaElement", "HTMLCanvasElement", "SVGElement", "SVGSVGElement", "Element", "Node", "Event", "KeyboardEvent", "MouseEvent", "MutationObserver", "getComputedStyle", "CustomEvent", "DOMRect", "Text", "Comment", "DocumentFragment", "requestAnimationFrame", "cancelAnimationFrame"]) {
  if (w[k] !== undefined && globalThis[k] === undefined) def(k, w[k]);
}
if (!globalThis.PointerEvent) globalThis.PointerEvent = w.MouseEvent;
globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} };
w.ResizeObserver = globalThis.ResizeObserver;
w.matchMedia = () => ({ matches: false, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {} });
globalThis.matchMedia = w.matchMedia;
Object.defineProperty(w.HTMLElement.prototype, "clientWidth", { get: () => 1200, configurable: true });
Object.defineProperty(w.HTMLElement.prototype, "clientHeight", { get: () => 800, configurable: true });
w.SVGElement.prototype.getBoundingClientRect = () => ({ left: 0, top: 0, width: 1200, height: 800, right: 1200, bottom: 800, x: 0, y: 0 });
// jsdom ne sait pas dessiner : xterm demande un contexte canvas au chargement.
w.HTMLCanvasElement.prototype.getContext = () => null;
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
globalThis.__SNAPSHOT_PATH = path.join(here, "fixtures", "snapshot.json");

const quiet = (msg) => typeof msg === "string" && (msg.includes("The above error") || msg.includes("React will try") || msg.startsWith("Warning:"));
const origError = console.error;
console.error = (...a) => { if (!quiet(a[0])) origError(...a); };

createRequire(import.meta.url)(out);
await globalThis.__run();
const failed = globalThis.__FAILED ?? 0;
if (failed > 0) {
  console.error(`\n${failed} écran(s) en échec.`);
  process.exit(1);
}
console.log("\nTous les écrans se montent sans erreur.");
// Les rafraîchissements périodiques (React Query) et la simulation du graphe
// garderaient la boucle d'évènements vivante : on sort explicitement.
process.exit(0);
