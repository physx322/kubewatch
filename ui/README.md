# Interface de KubeWatch

Interface React 19 + TypeScript, construite par Vite, embarquée dans le binaire
`kubewatch-desktop` (`crates/desktop`, backend Tauri 2) à la compilation. Le
backend expose des commandes typées ; l'interface ne parle jamais directement au
cluster.

## Commandes

```sh
npm install            # une fois (ou : make ui-install à la racine)
npm run typecheck      # tsc --noEmit
npm run build          # typecheck + bundle dans dist/
npm test               # test de fumée : chaque écran monté dans jsdom sur un instantané anonymisé (tests/smoke)
npm run dev            # serveur Vite seul (« make run » à la racine ouvre l'application avec)
```

`make run` exécute `cargo tauri dev` depuis `crates/desktop`. Les commandes
`beforeDevCommand` et `beforeBuildCommand` de `tauri.conf.json` sont déclarées
avec un `cwd` explicite (`../../ui`, relatif au fichier de configuration) : sans
cela, Tauri les lancerait depuis le dossier parent du crate, `crates/`.

## Arborescence

| Chemin | Rôle |
| --- | --- |
| `src/api/types.ts`, `hub.ts`, `updates.ts` | Types miroirs des structures Rust (camelCase) |
| `src/api/client.ts` | Une fonction typée par commande Tauri (`api.resources.list(…)`) ; erreurs en `Error` au message français ; flux via `Channel` |
| `src/app/store.ts` | État global (zustand, partiellement persisté) : écran, cluster, namespace, filtre, sélection, notifications |
| `src/app/queries.ts` | Hooks React Query partagés et cycle de vie (rechargement des clusters, avertissements du backend) |
| `src/app/shell/` | Barre latérale, barre supérieure, notifications |
| `src/components/` | Composants génériques : dialogue, confirmation, table triable, menu contextuel, éditeur YAML, Markdown |
| `src/views/` | Un fichier par écran |
| `src/assistant/` | Panneau de l'assistant IA et son store de conversation |
| `src/styles/tokens.css` | Jetons de design (couleurs, tailles, polices), thème clair/sombre |
| `src/styles/base.css` | Styles de base et classes utilitaires |

## Conventions

- **Français** partout dans l'interface, ton sobre : « Aucun objet », « lecture des évènements impossible ».
- **Jetons CSS**, jamais de couleur en dur : `var(--accent)`, `var(--fg-muted)`, `var(--border)`, `var(--bg-elev)`…
  Le thème sombre est automatique via les jetons.
- **Classes de `base.css`** avant tout CSS spécifique : `btn`, `btn-primary`, `btn-sm`, `input`, `select`, `card`, `panel`,
  `toolbar`, `tabs`/`tab`, `table`, `badge-ok|warn|err|info`, `tag`, `alert-*`, `empty`, `stats`/`stat`, `meter`.
- **Données** : `useQuery` avec une clé qui inclut le cluster (et le namespace si pertinent) ; mutations avec toast de
  succès/erreur ; `refetchInterval` conditionné à `useStore(s => s.autoRefresh)`.
- **Navigation entre écrans** : `useStore.getState().goToResource({ kind: "pods", name, namespace })` ouvre l'objet dans
  Ressources ; `openYamlConsole(yaml)` ouvre la console YAML avec un brouillon.
- **Filtre global** : la recherche de la barre supérieure est dans `useStore(s => s.filter)` ; chaque liste l'applique.
- **Raccourcis** : Ctrl+1…7 écrans, Ctrl+, réglages, Ctrl+J assistant, Ctrl+K recherche.

## Assistant IA

Le panneau (`src/assistant/`) envoie l'historique et un contexte (cluster, namespace, écran, objet sélectionné et son
YAML) à `ai_chat` ; les évènements de flux (`textDelta`, `toolCallStart`, `toolResult`, `turnEnd`…) alimentent des
« tours en cours » jusqu'à la réponse finale, puis l'historique est complété par les messages renvoyés (tours de
l'assistant et résultats d'outils). Les blocs YAML des réponses s'ouvrent d'un clic dans la console YAML.
