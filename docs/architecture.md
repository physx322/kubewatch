# Architecture

Comment KubeWatch est construit, et pourquoi il l'est ainsi.

- [Vue d'ensemble](#vue-densemble)
- [Les cinq crates](#les-cinq-crates)
- [Le pont : commandes et canaux](#le-pont--commandes-et-canaux)
- [Flux : lister des ressources](#flux--lister-des-ressources)
- [Flux : journaux et terminal](#flux--journaux-et-terminal)
- [Flux : détection d'une mise à jour](#flux--détection-dune-mise-à-jour)
- [État et persistance](#état-et-persistance)
- [Choix techniques](#choix-techniques)
- [Ce qui n'est délibérément pas là](#ce-qui-nest-délibérément-pas-là)

---

## Vue d'ensemble

```
             ┌───────────────────────────────────────────────┐
             │        binaire  kubewatch-desktop             │
             │   fenêtre Tauri 2 → webview du système        │
             └───────────────────────┬───────────────────────┘
                                     │
             ┌───────────────────────▼───────────────────────┐
             │   ui/   interface web  (React 19 + Vite)      │
             │   construite d'avance, embarquée dans le      │
             │   binaire à la compilation                    │
             └───────────────────────┬───────────────────────┘
                 invoke("…")         │        Channel<…>
                 commandes typées    │        journaux, terminal,
                 Result<T, String>   │        réponse de l'assistant
             ┌───────────────────────▼───────────────────────┐
             │           crate  desktop  (backend Tauri)     │
             │                                               │
             │  main.rs       construit l'application,       │
             │                déclare les commandes          │
             │  state.rs      AppState géré par Tauri        │
             │  commands/     app, clusters, resources,      │
             │                streams, hub, updater, ai      │
             │  assistant.rs  message système, outils        │
             │                de lecture du cluster          │
             │  util.rs       dossier d'état, rustls         │
             └───────────────────────┬───────────────────────┘
                                     │
    ┌──────────────┬─────────────────┴──────────────┬──────────────────┐
    │              │                                │                  │
┌───▼─────────┐ ┌──▼───────────────┐ ┌──────────────▼─────┐ ┌──────────▼──────┐
│kubewatch-   │ │ kubewatch-hub    │ │ kubewatch-updater  │ │ kubewatch-ai    │
│core         │ │                  │ │                    │ │                 │
│             │ │ registry images  │ │ github  releases   │ │ message commun  │
│client       │ │ charts ArtifactH │ │ policy  semver     │ │ anthropic       │
│discovery    │ │ catalog intégré  │ │ scan    inventaire │ │ openai (+compat)│
│cluster      │ │ deploy manifestes│ │ rollout set-image  │ │ sse   flux      │
│resource CRUD│ │ sizing empreinte │ │ store   état 0600  │ │ agent boucle    │
│apply SSA/dif│ │                  │ │ engine  orchestre  │ │ store  ai.json  │
│logs exec pf │ └────────┬─────────┘ └─────────┬──────────┘ └────────┬────────┘
│metrics event│          │                     │                     │
└──────┬──────┘          │                     │                     │
       │                 │                     │                     │
   kube-rs            reqwest               reqwest               reqwest
   (rustls)           (rustls)              (rustls)              (rustls)
       │                 │                     │                     │
       ▼                 ▼                     ▼                     ▼
┌──────────────┐ ┌────────────────┐ ┌──────────────────┐ ┌──────────────────┐
│ API server(s)│ │Docker Hub, GHCR│ │  api.github.com  │ │ api.anthropic.com│
│  Kubernetes  │ │Quay,ArtifactHub│ │                  │ │ api.openai.com   │
│              │ │                │ │                  │ │ serveur local    │
└──────────────┘ └────────────────┘ └──────────────────┘ └──────────────────┘
```

Le backend ne contient aucune logique Kubernetes : tout passe par les fonctions
de `core`, `hub` et `updater`, testables sans fenêtre. Une commande Tauri ne
fait rien de plus que résoudre le cluster, appeler l'une d'elles, et renvoyer le
résultat.

L'interface, elle, ne parle jamais au cluster : elle n'a ni client HTTP, ni
identifiants, ni accès au système de fichiers. Elle appelle des commandes et
reçoit des messages sur des canaux.

Aucun serveur, aucun port d'écoute, aucun agent dans le cluster, aucune ligne
de commande : c'est un client de poste de travail, au même titre que `kubectl`.

---

## Les cinq crates

### `kubewatch-core` — tout ce qui parle à Kubernetes

| Module | Rôle |
| --- | --- |
| `client` | Établit la connexion : kubeconfig, YAML en ligne, URL + jeton, in-cluster. Lit les contextes, la version du serveur, `/healthz` |
| `discovery` | Construit le catalogue des types réellement exposés par l'API server et résout un nom d'usage (`po`, `deploy`, `ingresses.networking.k8s.io`) |
| `cluster` | `ClusterHandle` (client + catalogue + namespace) et `ClusterManager` (plusieurs clusters, persistance, cluster courant) |
| `resource` | CRUD générique sur `DynamicObject`, résumé d'objet, scale, restart, set-image, rollback, cordon, drain |
| `apply` | Découpage multi-documents, Server-Side Apply, diff, delete |
| `logs`, `exec`, `portforward` | Flux de journaux, session interactive, redirection de port |
| `metrics`, `events` | `metrics.k8s.io`, vue d'ensemble, évènements récents |

Le point central est la **découverte dynamique** : aucun type n'est codé en dur.
Une CRD installée hier apparaît aujourd'hui, avec ses verbes, ses abréviations
et ses catégories.

### `kubewatch-hub` — registres, charts, catalogue

Recherche et inspection d'images sur Docker Hub, GHCR, Quay et tout registre OCI
conforme (manifeste et configuration lus directement) ; recherche de charts sur
Artifact Hub ; catalogue intégré d'applications ; générateur de manifestes
(Deployment, Service, Ingress, PVC) à partir d'une description simple.

Le module `sizing` complète le générateur : profils de taille, empreinte d'un
déploiement (demandes et limites × répliques, stockage) et confrontation à la
capacité du cluster, verdict et remarques compris. Il ne touche ni au réseau ni
à l'interface : l'écran « Déployer » ne fait qu'afficher ce qu'il calcule.

### `kubewatch-updater` — détection et déploiement

| Module | Rôle |
| --- | --- |
| `github` | Releases, tags, quota restant ; normalisation des étiquettes en semver |
| `policy` | Canaux, contraintes, exclusions, fenêtre de maintenance |
| `scan` | Inventaire des images en service, suggestion de surveillances |
| `rollout` | Pose de l'image, suivi du rollout, retour arrière |
| `store` | État sur disque, écriture atomique, permissions `0600` |
| `engine` | Orchestration : vérification, application, historique |

Le module `webhook` (vérification HMAC-SHA256 d'une livraison GitHub) existe
toujours dans la bibliothèque, mais **plus aucun binaire ne l'appelle** : il n'y
a plus de serveur HTTP pour recevoir la requête. Voir
[updates.md](updates.md#le-webhook-github-a-été-retiré).

### `kubewatch-ai` — l'assistant, sans rien savoir de Kubernetes

Un modèle de conversation commun (`ChatMessage`, `Part` : texte, appel d'outil,
résultat d'outil), deux dialectes de fournisseur — l'API Messages d'Anthropic
et l'API Chat Completions d'OpenAI, ce second dialecte servant aussi aux
serveurs locaux compatibles (LM Studio, Ollama, llama.cpp, Jan…) — un
analyseur SSE incrémental, et une boucle d'agent : le modèle répond en flux,
demande des outils, l'application les exécute, les résultats repartent, jusqu'à
la réponse finale ou au plafond de tours. Les outils sont décrits par un
`ToolSpec` et exécutés par un `ToolExecutor` fourni par l'application : le
crate ne dépend d'aucun autre crate de KubeWatch. Les profils (fournisseur,
adresse, modèle, clé) vivent dans `ai.json`, en `0600` ; la clé n'est jamais
renvoyée à l'interface, seule sa présence l'est.

### `kubewatch-desktop` — l'application

Le seul binaire du projet, `kubewatch-desktop` : un backend Tauri 2 qui embarque
l'interface web de `ui/`.

| Module | Rôle |
| --- | --- |
| `main` | Installe le fournisseur rustls et les traces, construit l'application Tauri, déclare les commandes, recharge les clusters en tâche de fond, ferme les flux à la destruction de la fenêtre |
| `state` | `AppState`, géré par Tauri et injecté dans chaque commande : `ClusterManager`, `Store`, `HubClient`, `UpdateEngine`, `AiStore`, flux ouverts, conversations en cours |
| `commands/` | Les commandes exposées à l'interface, par domaine : `app`, `clusters`, `resources`, `streams`, `hub`, `updater`, `ai` |
| `assistant` | Le message système de l'assistant et ses sept outils de lecture du cluster, implémentés sur `ToolExecutor` |
| `util` | Dossier d'état, fournisseur cryptographique, troncature et mise en forme des âges |

`crates/desktop/tauri.conf.json` décrit la fenêtre (1440 × 900, minimum
960 × 620, centrée), l'identifiant `io.kubewatch.KubeWatch`, la CSP, les cibles
d'empaquetage (AppImage, `.deb`, `.rpm`) et les deux commandes npm que
`tauri-cli` lance avant de compiler ou de démarrer. `capabilities/default.json`
n'accorde à la fenêtre `main` que `core:default` : aucun plugin Tauri
supplémentaire n'est activé.

### `ui/` — l'interface

Une application **React 19 + TypeScript** construite par **Vite**. Elle est
compilée d'avance dans `ui/dist`, que `tauri::generate_context!` embarque dans
le binaire : l'exécutable livré ne lit aucun fichier d'interface sur le disque.

| Chemin | Rôle |
| --- | --- |
| `src/api/client.ts` | Une fonction typée par commande ; les erreurs deviennent des `Error` au message français ; les flux passent par un `Channel` |
| `src/api/types.ts`, `hub.ts`, `updates.ts` | Types miroirs des structures Rust, en camelCase |
| `src/app/store.ts` | État global (zustand), partiellement persisté dans le `localStorage` de la webview |
| `src/app/queries.ts` | Hooks React Query partagés et cycle de vie du démarrage |
| `src/app/shell/` | Barre latérale, barre supérieure, notifications |
| `src/views/` | Un fichier par écran |
| `src/assistant/` | Le panneau de l'assistant et son état de conversation |
| `src/components/` | Table triable, dialogue, confirmation, menu, éditeur YAML (CodeMirror), terminal (xterm.js), rendu Markdown |

---

## Le pont : commandes et canaux

C'est la pièce centrale : tout ce que fait l'interface passe par là, et rien
d'autre ne la relie au cœur Rust.

**Le problème.** L'interface tourne dans une webview, dans son propre processus,
en JavaScript. Le cœur Kubernetes est en Rust, asynchrone, et lent à l'échelle
humaine : une liste de pods peut prendre 300 ms, un cluster injoignable peut
mettre trente secondes avant d'échouer. Les deux ne partagent aucune mémoire.

**La solution.** Deux mécanismes, et deux seulement.

```
        interface (webview, JS)              backend Tauri (tokio)
   ─────────────────────────────         ─────────────────────────────
                                 invoke
    api.resources.list(…)  ───────────────▶  #[tauri::command] async fn
              │               une requête IPC          │
              │                                        │ core / hub / updater
              │                                        ▼
    await → T   ◀───────────────────────────  Result<T, String>
              │            réponse ou message d'erreur


    api.logs.start(…, onEvent) ──────────▶  tâche tokio
              │                                        │
              │            Channel<LogEvent>           ▼
    onEvent(ev) ◀───────────────────────────  on_event.send(LogEvent::…)
              │                                        │
    api.logs.stop(id) ───────────────────▶  AppState::abort_stream(id)
```

### Les commandes

Chaque commande est une fonction annotée `#[tauri::command]`, déclarée dans
`tauri::generate_handler![…]` et appelée depuis l'interface par `invoke`.
`crates/desktop/src/commands/mod.rs` en fixe les conventions :

- chaque commande renvoie `Result<T, String>` ; l'erreur est **un message
  français prêt à afficher**, pas un type à interpréter ;
- un argument `cluster` vide désigne le cluster courant ;
- les arguments et les résultats sont sérialisés en JSON, avec les mêmes noms
  qu'en TypeScript (camelCase).

Le catalogue va de `app_info` à `ai_cancel`, en passant par `list_resources`,
`apply_yaml`, `diff_yaml`, `drain_node`, `hub_deploy` ou `updates_check` : une
commande par action de l'interface, sans commande fourre-tout.

### L'`AppState`

`AppState::new` est construit au démarrage et confié à Tauri (`app.manage`), qui
l'injecte ensuite dans chaque commande sous la forme d'un `State<'_, AppState>`.
Il réunit le `ClusterManager`, le `Store` du détecteur de mises à jour, le
`HubClient`, l'`UpdateEngine`, l'`AiStore`, les flux ouverts et les
conversations en cours.

Deux propriétés méritent d'être notées :

1. **Aucune entrée/sortie réseau au démarrage.** `AppState::new` ne fait que
   lire les fichiers d'état. La reconnexion aux clusters enregistrés part dans
   une tâche de fond, parce qu'un cluster injoignable met jusqu'à trente
   secondes à répondre et que la fenêtre doit s'afficher tout de suite. Quand
   elle aboutit, le backend émet l'évènement `clusters-changed` et l'interface
   recharge sa liste.
2. **Les défaillances du démarrage ne tuent rien.** Un fichier d'état illisible,
   un client HTTP qui ne se construit pas : chaque cas ajoute un message à
   `startup_warnings`, que la première commande `app_info` remet à l'interface,
   qui l'affiche en notification. `hub` et `engine` sont des `Option` ; les
   commandes qui en dépendent renvoient une erreur explicite au lieu de
   paniquer. Toute l'interface reste utilisable sans qu'aucun cluster ne soit
   joignable, et c'est une exigence, pas un effet de bord.

### Les canaux

Trois choses ne tiennent pas dans une réponse unique : les journaux, la session
interactive et la réponse de l'assistant. Elles passent par un **`Channel`**
Tauri, créé par l'interface et transmis en argument de la commande qui démarre
le flux. Le backend y pousse des évènements sérialisés ; l'interface les reçoit
dans le rappel qu'elle a fourni.

```rust
#[tauri::command]
pub async fn start_logs(
    app: AppHandle,
    state: State<'_, AppState>,
    cluster: String,
    pod: ResourceRef,
    opts: LogOptions,
    on_event: Channel<LogEvent>,
) -> Result<u64, String>
```

### Les identifiants

Un flux qui démarre renvoie un **identifiant**, tiré d'un compteur atomique de
l'`AppState`, et l'entrée correspondante est rangée dans
`AppState::streams` avec le `AbortHandle` de sa tâche. L'interface se sert de
cet identifiant pour tout le reste : `exec_input`, `exec_resize`, `stop_logs`,
`stop_exec`. `abort_stream` interrompt la tâche, et la session interactive avec
elle.

C'est ce qui rend les flux **interruptibles sans ambiguïté** : ouvrir les
journaux d'un pod puis ceux d'un autre ne mélange pas les deux, parce que chaque
flux écrit dans son propre canal et s'arrête sur son propre identifiant.
L'assistant suit la même règle, avec un `chat_id` choisi par l'interface et
rendu par `ai_cancel`.

Quand la fenêtre est détruite, `AppState::abort_all` ferme tous les flux et
toutes les conversations : rien ne survit à la fermeture.

---

## Flux : lister des ressources

L'utilisateur choisit le type « deployments » dans l'écran Ressources.

```
interface      useQuery({
                 queryKey: ["resources", cluster, "deployments", namespace, selector],
                 queryFn:  () => api.resources.list(cluster, "deployments", opts),
               })
               └─ invoke("list_resources", { cluster, kind, opts })

backend        #[tauri::command] list_resources
               ├─ state.handle(&cluster)        -> ClusterHandle
               ├─ handle.resolve_kind("deployments") -> ApiResource
               ├─ resource::list(&handle, &ar, &opts).await
               └─ Ok(ObjectListPage { items, continueToken, … })

interface      React Query range la page sous sa clé, le tableau se redessine
```

La **clé de requête** joue le rôle que tenait autrefois un identifiant de
requête : une réponse arrivée en retard est rangée sous sa propre clé, jamais
par-dessus une autre. Changer de type, de cluster ou de namespace change la
clé, donc la réponse tardive du type précédent n'écrase rien.

Le tri et le filtre s'appliquent **localement**, sur les lignes déjà reçues :
taper dans le champ de recherche ne déclenche aucune requête. Le sélecteur de
labels, lui, fait partie de la clé et part au serveur, parce que c'est l'API qui
sait l'évaluer.

Le rafraîchissement automatique est le `refetchInterval` de React Query,
conditionné au commutateur de la barre supérieure ; il renvoie simplement la
même commande à intervalle régulier, sans jamais doubler une requête déjà en
vol.

---

## Flux : journaux et terminal

Ce sont les seuls flux **continus** côté cluster, et le pont les traite comme
tout le reste : une commande pour ouvrir, un canal pour recevoir, un identifiant
pour arrêter.

```
api.logs.start(cluster, pod, opts, onEvent)  ──▶  start_logs -> id
    tâche tokio : logs::stream(&handle, &pod, &opts)
      pour chaque lot :  LogEvent::Lines { lines }
      à la fin        :  LogEvent::Ended { error: Option<String> }

api.logs.stop(id)                            ──▶  stop_logs(id) : la tâche est
                                                  interrompue, l'entrée retirée
```

Les lignes sont **regroupées par lots** : au plus une émission toutes les 40 ms,
ou dès que 500 lignes se sont accumulées. Sans cela, un pod bavard saturerait le
pont IPC à lui seul. L'interface refait le même geste de son côté, avec un
tampon borné à 20 000 lignes : la mémoire ne croît pas indéfiniment, et le
défilement automatique se décroche dès qu'on remonte dans l'historique.

Le terminal fonctionne dans les deux sens. `start_exec` ouvre la session avec un
TTY et renvoie son identifiant ; `exec_input` pousse les frappes, encodées en
base64 ; `exec_resize` transmet la nouvelle taille de la grille ;
`ExecEvent::Output` rapporte ce que le conteneur écrit, en base64 lui aussi ;
`ExecEvent::Ended` clôt la session. L'émulation ANSI est celle de **xterm.js**,
côté interface : le backend ne fait que transporter des octets.

---

## Flux : détection d'une mise à jour

```
api.updates.check()  ──▶  updates_check
   └─ UpdateEngine::check_all()
        pour chaque surveillance activée :
          1. lit l'objet ciblé dans le cluster, extrait l'image du conteneur
          2. normalise l'étiquette en semver (v1.2.3, release-1.2.3, 1.2.3-alpine…)
          3. interroge la source : releases GitHub, tags du registre, ou chart
          4. la politique tranche : canal, contrainte, exclusions
          5. si autoApply et fenêtre de maintenance ouverte : rollout::apply()
   ◀─ Vec<UpdateFinding>
```

C'est une commande ordinaire : elle rend la main quand tout est fini, et
l'interface affiche un indicateur d'activité pendant ce temps. Rien n'est
poussé sur un canal, parce qu'il n'y a qu'un résultat à rendre.

Les déploiements automatiques sont **séquentiels** : deux rollouts simultanés
sur le même cluster rendraient les diagnostics illisibles. Un échec est
journalisé sans interrompre les suivants.

Il n'y a pas de boucle d'ordonnancement dans le produit : une vérification part
du bouton de l'écran Mises à jour, et de lui seul — voir
[updates.md](updates.md#vérifications-régulières).

---

## État et persistance

| Élément | Où | Format |
| --- | --- | --- |
| Clusters enregistrés | `<state_dir>/clusters.json` | JSON, `0600`, écriture atomique |
| Surveillances, détections, historique, jetons | `<state_dir>/updater.json` | JSON, `0600`, écriture atomique |
| Profils et réglages de l'assistant | `<state_dir>/ai.json` | JSON, `0600`, écriture atomique |
| Préférences de l'interface (thème, cluster, namespace par cluster, écran, rafraîchissement, panneau de l'assistant) | `localStorage` de la webview, clé `kubewatch-ui` | JSON |
| Catalogue des types | mémoire | `Arc<RwLock<ResourceCatalog>>` par cluster |
| Sessions logs et exec | mémoire | Tâches tokio, canaux Tauri |
| Conversations de l'assistant | mémoire | Perdues à la fermeture |
| `AppState` | mémoire | Reconstruit à chaque démarrage à partir des fichiers ci-dessus |

Aucune base de données, aucun serveur d'état externe : trois fichiers JSON
suffisent. Corollaire assumé : **une seule instance à la fois** écrit ces
fichiers. Lancer deux fois l'application et enregistrer depuis les deux peut
faire perdre la modification la plus ancienne ; l'écriture atomique
(fichier temporaire puis renommage) garantit seulement qu'aucun fichier ne sera
jamais tronqué.

La géométrie de la fenêtre n'est pas conservée : `tauri.conf.json` fixe une
taille de départ et la fenêtre s'ouvre centrée à chaque lancement.

---

## Choix techniques

### Une webview, et ce qu'elle coûte

KubeWatch affiche son interface dans une **webview** pilotée par Tauri 2. Ce
choix a un prix, et il est réel.

Ce qu'il coûte :

- **une dépendance à webkit2gtk sous Linux**, liée à la compilation et exigée à
  l'exécution. Le binaire lie `libwebkit2gtk-4.1`, `libjavascriptcoregtk-4.1`,
  `libsoup-3.0`, `libgtk-3`, `libgdk-3`, `libgdk_pixbuf-2.0`, `libcairo`,
  `libglib-2.0`, `libgobject-2.0`, `libgio-2.0` et `libdbus-1`. Ce ne sont pas
  des bibliothèques ouvertes à l'exécution que l'on peut ignorer : leur absence
  empêche déjà la compilation. Les paquets sont énumérés dans
  [installation.md](installation.md#dépendances-de-compilation) et
  [packaging/README.md](../packaging/README.md#dépendances-dexécution) ;
- **une chaîne d'approvisionnement npm à auditer**, en plus de celle de Cargo.
  L'interface dépend de React, Vite, React Query, zustand, CodeMirror, xterm.js,
  react-markdown, d3-force et des icônes Phosphor. `package-lock.json` fige les
  versions, mais l'écosystème reste ce qu'il est ;
- **pas de binaire musl**, pour la même raison qu'avant, mais par un autre
  chemin : la webview et GTK sont des bibliothèques partagées du système.

Ce qu'il apporte, et c'est ce pour quoi il a été retenu :

- **le Markdown de l'assistant**, rendu correctement — listes, tableaux, blocs
  de code coloriés, avec un bouton qui ouvre un bloc YAML dans la console ;
- **un éditeur de code complet** (CodeMirror : coloration YAML, pliage,
  sélection multiple) plutôt qu'une zone de texte ;
- **un vrai émulateur de terminal** (xterm.js), qui gère les séquences ANSI, les
  applications plein écran et le redimensionnement ;
- **une itération rapide sur l'interface** : `make run` ouvre la fenêtre avec le
  rechargement à chaud de Vite ; changer un écran ne recompile pas le Rust.

La contrepartie est contenue par la configuration, pas par la confiance : la
webview ne charge **aucun contenu distant**. La CSP déclarée dans
`tauri.conf.json` restreint `default-src` et `script-src` à `'self'`, et
`connect-src` au seul canal IPC. Voir
[security.md](security.md#la-webview-et-son-contenu).

### Tauri 2 plutôt qu'un navigateur embarqué

Tauri utilise la webview **fournie par le système** au lieu d'en embarquer une.
Le binaire n'inclut donc pas de moteur de rendu : il s'y lie. Sous Linux, c'est
WebKitGTK ; la CI ne pose aucun paquet système sur macOS ni sur Windows, où la
webview fait partie du système.

Conséquence assumée : le rendu dépend de la version de WebKitGTK installée.
`vite.config.ts` cible en conséquence `safari13` partout sauf sous Windows, et
`chrome105` sous Windows.

### Le runtime asynchrone est celui de Tauri

Il n'y a plus de runtime tokio construit et gardé en vie à la main. Tauri en
fournit un : les commandes `async` s'y exécutent, `tauri::async_runtime::spawn`
y dépose les tâches de démarrage, et `tokio::spawn` celles des flux. Une
commande `async` qui attend trente secondes n'immobilise que sa propre tâche ;
la fenêtre, elle, est dans un autre processus.

### rustls plutôt qu'OpenSSL

Aucune dépendance système pour TLS : pas de `libssl.so.1.1 not found`, pas de
recompilation à chaque mise à jour d'OpenSSL.

`kube` compile rustls avec `ring`, `reqwest` avec `aws-lc-rs` : deux
fournisseurs cohabitent dans le binaire, et rustls refuse alors de choisir seul.
Le fournisseur doit donc être installé explicitement **au démarrage, avant toute
connexion TLS** — faute de quoi la première requête panique. C'est la toute
première instruction de `main()`, avant même l'initialisation des traces.

### Profil release optimisé pour la taille

```toml
[profile.release]
opt-level = "z"      # optimiser la taille plutôt que la vitesse
lto = "fat"          # optimisation inter-modules complète
codegen-units = 1    # une seule unité : plus lent à compiler, plus compact
panic = "abort"      # pas de machinerie de déroulement
strip = "symbols"    # ni symboles ni informations de débogage
```

Un tableau de bord n'est pas un calculateur : le temps passé à attendre l'API
server domine largement tout gain d'`opt-level = 3`. La CI mesure le binaire à
chaque build et échoue au-delà d'un budget fixé dans le workflow.

### Erreurs typées, traduites une seule fois

Chaque crate définit son `Error` (`thiserror`) avec sa chaîne de causes. La
commande Tauri les convertit en `String` — la seule frontière où la traduction
a lieu — et l'interface les relève en `Error` JavaScript, affichées en
notification. La logique métier n'a jamais à savoir où son erreur sera lue.

### `#![forbid(unsafe_code)]`

Dans les cinq crates. Une console d'administration n'a aucune raison de
manipuler de la mémoire non vérifiée.

---

## Ce qui n'est délibérément pas là

| Absent | Pourquoi |
| --- | --- |
| Serveur HTTP, API REST, interface web servie sur le réseau | Retirés au profit de l'application de bureau : moins de surface d'écoute, aucune authentification à réinventer, aucun jeton à protéger. L'interface est du HTML, mais elle ne quitte jamais le binaire |
| Contenu distant dans la webview | La CSP restreint `default-src` et `script-src` à `'self'` et `connect-src` au canal IPC : aucune page, aucun script et aucune ressource ne vient du réseau |
| Base de données | Trois fichiers JSON suffisent ; une base briserait l'autonomie du binaire |
| Comptes utilisateurs | Le contrôle d'accès de Kubernetes, c'est le RBAC ; en dupliquer une version approximative créerait une fausse sécurité |
| Moteur de gabarits Helm | Helm existe et fait autorité ; KubeWatch délègue à son binaire plutôt que d'en produire une imitation divergente |
| Agent dans le cluster | L'API server expose déjà tout ce qui est nécessaire |
| Ordonnanceur intégré | Il vivait dans le serveur ; systemd, cron et le Planificateur de tâches font ce travail mieux et survivent aux redémarrages |
| Binaire musl de l'application | La webview, GTK et leurs dépendances sont des bibliothèques partagées du système : l'artefact statique serait inutilisable |
| Écriture dans le cluster par l'assistant | Ses sept outils sont en lecture seule ; il propose un manifeste ou une commande, c'est vous qui l'appliquez |
| Télémétrie | Aucune donnée ne quitte votre machine, sauf vers votre cluster et vers les services que vous interrogez explicitement |
