# Architecture

Comment KubeWatch est construit, et pourquoi il l'est ainsi.

- [Vue d'ensemble](#vue-densemble)
- [Les quatre crates](#les-quatre-crates)
- [Le pont asynchrone](#le-pont-asynchrone)
- [Flux : lister des ressources](#flux--lister-des-ressources)
- [Flux : journaux et terminal](#flux--journaux-et-terminal)
- [Flux : détection d'une mise à jour](#flux--détection-dune-mise-à-jour)
- [État et persistance](#état-et-persistance)
- [Choix techniques](#choix-techniques)
- [Ce qui n'est délibérément pas là](#ce-qui-nest-délibérément-pas-là)

---

## Vue d'ensemble

```
                 ┌──────────────────────────────┐
                 │   binaire kubewatch-desktop  │
                 │   fenêtre native (egui)      │
                 └───────────────┬──────────────┘
                                 │
                 ┌───────────────▼──────────────┐
                 │      crate  desktop          │
                 │                              │
                 │  app.rs      boucle eframe   │
                 │  state.rs    AppState, vues  │
                 │  views/      8 écrans        │
                 │  widgets/    toasts, confirm,│
                 │              logs, terminal, │
                 │              éditeur YAML    │
                 │  icons.rs    police Phosphor │
                 │  backend/    LE PONT         │
                 │    ├ command.rs  Command     │
                 │    ├ event.rs    Event       │
                 │    └ worker.rs   runtime     │
                 └───────────────┬──────────────┘
                                 │
    ┌────────────────────────────┴───────────────────────────────────────┐
    │                                                                    │
   ┌▼────────────────┐   ┌──────────────────┐   ┌────────────────────┐
   │ kubewatch-core  │   │  kubewatch-hub   │   │ kubewatch-updater  │
   │                 │   │                  │   │                    │
   │ client connexion│   │ registry images  │   │ github  releases   │
   │ discovery types │   │ charts ArtifactH │   │ policy  semver     │
   │ cluster handles │   │ catalog intégré  │   │ store   état 0600  │
   │ resource CRUD   │   │ deploy manifests │   │ scan    inventaire │
   │ apply  SSA/diff │   │                  │   │ rollout set-image  │
   │ logs exec pf    │   └────────┬─────────┘   │ engine  orchestre  │
   │ metrics events  │            │             └─────────┬──────────┘
   └────────┬────────┘            │                       │
            │                     │                       │
        kube-rs                reqwest                 reqwest
        (rustls)               (rustls)                (rustls)
            │                     │                       │
            ▼                     ▼                       ▼
   ┌─────────────────┐   ┌──────────────────┐   ┌────────────────────┐
   │  API server(s)  │   │ Docker Hub, GHCR │   │   api.github.com   │
   │   Kubernetes    │   │ Quay, ArtifactHub│   │                    │
   └─────────────────┘   └──────────────────┘   └────────────────────┘
```

L'application ne contient aucune logique Kubernetes : tout passe par les
fonctions de `core`, `hub` et `updater`, testables sans fenêtre. Un bouton ne
fait rien de plus qu'appeler l'une d'elles depuis le fil du backend.

Aucun serveur, aucun port d'écoute, aucun agent dans le cluster, aucune ligne
de commande : c'est un client de poste de travail, au même titre que `kubectl`.

---

## Les quatre crates

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

### `kubewatch-desktop` — l'application

| Module | Rôle |
| --- | --- |
| `app` | Implémente `eframe::App` : `logic()` draine les évènements, `ui()` dessine |
| `state` | `AppState` : la totalité de ce qui est affiché, plus les sous-états par écran |
| `views/` | Huit écrans (`overview`, `resources`, `graph`, `yaml`, `deploy`, `hub`, `updates`, `settings`) et le panneau de détail |
| `widgets/` | Notifications, boîtes de confirmation, vue de journaux, terminal, éditeur YAML |
| `backend/` | Le pont asynchrone : `Command`, `Event`, et le `Backend` qui les relie |
| `theme`, `format`, `icons` | Palette, mise en forme (durées, quantités, âges) et icônes (police Phosphor, nommées par rôle) |

---

## Le pont asynchrone

C'est la pièce centrale, et la seule vraiment nouvelle depuis le pivot.

**Le problème.** egui est *immediate mode* et synchrone : à chaque image, la
totalité de l'interface est redessinée par un appel de fonction qui doit rendre
la main en quelques millisecondes. `kube`, lui, est asynchrone et lent à
l'échelle humaine : une liste de pods peut prendre 300 ms, un cluster
injoignable peut prendre 30 secondes avant d'échouer. Appeler l'un depuis
l'autre — même une seule fois, même « juste pour essayer » — gèlerait la fenêtre.

**La solution.** Deux files de messages et un runtime tokio qui vit à côté.

```
        fil d'interface (egui)                 runtime tokio (N fils)
   ─────────────────────────────          ─────────────────────────────
                                    Command
    backend.send(Command::…)  ──────────────────▶  une tâche par commande
              │                  (mpsc unbounded)          │
              │                                            │ core / hub / updater
              │                                            ▼
    backend.drain() ◀───────────────────────────  tx.send(Event::…)
              │                   Event                    │
              │              (std::sync::mpsc)             │
              │                                            ▼
    applique sur AppState                      ctx.request_repaint()
              │                                    réveille la fenêtre
              ▼
    views::*::show(ui, &mut st, &backend)
```

```rust
pub struct Backend {
    tx: tokio::sync::mpsc::UnboundedSender<Command>,
    rx: std::sync::mpsc::Receiver<Event>,
    rt: tokio::runtime::Runtime,   // gardé en vie pour la durée de l'application
}

impl Backend {
    pub fn new(ctx: egui::Context, state_dir: std::path::PathBuf) -> anyhow::Result<Self>;
    pub fn send(&self, cmd: Command);    // ne bloque jamais
    pub fn drain(&self) -> Vec<Event>;   // vide la file, appelé une fois par image
}
```

Quatre propriétés, et chacune compte :

1. **`send` ne bloque jamais.** La file de commandes est non bornée : déposer un
   message est une écriture en mémoire. Si le cluster met trente secondes à
   répondre, c'est la tâche tokio qui attend, pas la fenêtre.
2. **`drain` ne bloque jamais non plus.** Il vide ce qui est arrivé et rend la
   main immédiatement. Appelé une fois par image, dans `logic()`, avant tout
   dessin.
3. **Le worker réveille la fenêtre.** `egui::Context` est `Send + Sync` et
   clonable. Après chaque `Event` émis, le worker appelle
   `ctx.request_repaint()` : l'application dort tant qu'il ne se passe rien, et
   se réveille à la milliseconde où une réponse arrive. Pas de boucle à 60 Hz
   qui tourne dans le vide, pas de sondage.
4. **Chaque commande longue porte un `RequestId`**, repris dans l'`Event`
   correspondant. L'interface ignore les réponses dont l'identifiant n'est plus
   celui qu'elle attend. Sans cela, changer d'écran pendant qu'une requête est
   en vol ferait apparaître, quelques secondes plus tard, la liste des pods
   par-dessus l'écran du hub. `AppState::pending` associe chaque identifiant en
   vol à son libellé, ce qui alimente l'indicateur d'activité.

**Le worker ne panique jamais.** Chaque exécution de commande est enveloppée et
ses erreurs sont converties en `Event::Failed { id, message }`, avec un message
français lisible. Une commande qui échoue affiche une notification ; elle ne
tue ni le fil, ni la file, ni l'application. Le principe vaut aussi pour
l'absence de cluster : toute l'interface reste utilisable sans qu'aucun cluster
ne soit joignable, et c'est une exigence, pas un effet de bord.

**Les vues ne font aucun appel réseau.** Leur signature l'interdit :

```rust
pub fn show(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend)
```

Une vue lit `st`, dessine, et envoie éventuellement une `Command`. Elle n'a
accès ni à un client Kubernetes, ni à un runtime, ni à un `await`. La règle est
tenue par le type, pas par la discipline.

---

## Flux : lister des ressources

L'utilisateur choisit le type « deployments » dans l'écran Ressources.

```
image N        views::resources::show
               └─ le ComboBox change de valeur
                  st.selected_kind = "deployments"
                  let id = st.next_id();
                  st.pending.insert(id, "Liste des deployments");
                  backend.send(Command::ListResources { id, cluster, kind, opts })
               (l'image se termine normalement : rien n'a bloqué)

               ┌ runtime tokio ──────────────────────────────────────┐
               │ cluster.resolve("deployments")  -> ApiResource      │
               │ resource::list(&handle, &ar, &opts).await           │
               │ tx.send(Event::Resources { id, cluster, kind, page})│
               │ ctx.request_repaint()                              │
               └────────────────────────────────────────────────────┘

image N+k      app::logic
               └─ backend.drain() -> [Event::Resources { id, … }]
                  si st.pending.shift_remove(&id).is_some() {
                      st.rows = page.items;     // sinon : réponse obsolète,
                  }                             // ignorée en silence
image N+k      app::ui  -> le tableau se redessine avec les nouvelles lignes
```

Le tri et le filtre (`st.filter`, `st.sort`) s'appliquent **localement**, sur
les lignes déjà reçues : taper dans le champ de recherche ne déclenche aucune
requête. Le sélecteur de labels (`st.label_selector`), lui, part au serveur,
parce que c'est l'API qui sait l'évaluer.

Le rafraîchissement automatique (`st.auto_refresh`, `st.refresh_every`,
`st.last_refresh`) renvoie simplement la même commande à intervalle régulier.
S'il reste déjà une requête du même type en vol, elle n'est pas doublée.

---

## Flux : journaux et terminal

Ce sont les seuls flux **continus**, et le pont les traite comme n'importe quoi
d'autre : une commande, puis une suite d'évènements portant le même identifiant.

```
Command::StartLogs { id, cluster, pod, opts }
   └─ tâche tokio : logs::stream(&handle, &pod, &opts)
        pour chaque ligne :  Event::LogLine { id, line }  + request_repaint()
        à la fin         :  Event::LogEnded { id }

Command::StopLogs { id }
   └─ la tâche est annulée ; l'interface cesse d'accepter les LogLine de cet id
```

Le terminal fonctionne de la même façon, dans les deux sens :
`Command::StartExec` ouvre la session, `Command::ExecInput` pousse les frappes,
`Command::ExecResize` transmet la nouvelle taille du TTY, `Event::ExecOutput`
rapporte ce que le conteneur écrit, et `Event::ExecEnded` clôt la session.

Le `RequestId` joue ici son rôle le plus visible : si vous ouvrez les journaux
d'un pod, puis ceux d'un autre, les lignes en retard du premier portent l'ancien
identifiant et sont jetées. Sans cela, les deux flux se mélangeraient à l'écran.

La vue de journaux conserve un tampon borné : un pod bavard ne fait pas croître
la mémoire indéfiniment, et le défilement automatique se coupe dès que vous
remontez dans l'historique.

---

## Flux : détection d'une mise à jour

```
Command::UpdatesCheck { id }
   └─ UpdateEngine::check_all()
        pour chaque surveillance activée :
          1. lit l'objet ciblé dans le cluster, extrait l'image du conteneur
          2. normalise l'étiquette en semver (v1.2.3, release-1.2.3, 1.2.3-alpine…)
          3. interroge la source : releases GitHub, tags du registre, ou chart
          4. la politique tranche : canal, contrainte, exclusions
          5. si autoApply et fenêtre de maintenance ouverte : rollout::apply()
        Event::Findings { id, findings }
```

Les déploiements automatiques sont **séquentiels** : deux rollouts simultanés
sur le même cluster rendraient les diagnostics illisibles. Un échec est
journalisé sans interrompre les suivants.

Il n'y a plus de boucle d'ordonnancement dans le produit : une vérification part
du bouton de l'écran Mises à jour, et de lui seul — voir
[updates.md](updates.md#vérifications-régulières).

---

## État et persistance

| Élément | Où | Format |
| --- | --- | --- |
| Clusters enregistrés | `<state_dir>/clusters.json` | JSON, `0600`, écriture atomique |
| Surveillances, détections, historique, jetons | `<state_dir>/updater.json` | JSON, `0600`, écriture atomique |
| Préférences de l'application (thème, zoom, écran courant, cluster courant) | stockage `eframe` | RON, écrit à la fermeture |
| Géométrie de la fenêtre, état des panneaux | stockage `eframe` (feature `persistence`) | RON, même fichier |
| Catalogue des types | mémoire | `Arc<RwLock<ResourceCatalog>>` par cluster |
| Sessions logs et exec | mémoire | Tâches tokio, canaux `mpsc` |
| `AppState` | mémoire | Reconstruit à chaque démarrage à partir des fichiers ci-dessus |

Aucune base de données, aucun serveur d'état externe : deux fichiers JSON
suffisent. Corollaire assumé : **une seule instance à la fois** écrit ces
fichiers. Lancer deux fois l'application et enregistrer depuis les deux peut
faire perdre la modification la plus ancienne ; l'écriture atomique
(fichier temporaire puis renommage) garantit seulement qu'aucun fichier ne sera
jamais tronqué.

---

## Choix techniques

### egui plutôt qu'une webview

Une webview (Tauri, Electron, wry) aurait imposé un moteur de rendu HTML —
50 à 150 Mio de dépendances système sous Linux, une surface d'attaque
considérable, et une chaîne d'approvisionnement JavaScript à auditer. egui rend
avec le GPU, tient dans le binaire, et n'a besoin d'aucun navigateur installé.
Il n'y a **pas** de webkit2gtk dans les dépendances, et c'est délibéré.

Le mode immédiat a un coût : chaque image redessine tout, donc tout doit être
rapide. C'est précisément ce qui rend le pont asynchrone obligatoire, et donc ce
qui garantit qu'aucune opération réseau ne peut se glisser dans le fil
d'interface — l'architecture interdit la faute au lieu de la surveiller.

### eframe 0.36, backend wgpu, Wayland et X11

`wgpu` cible Vulkan, Metal, Direct3D 12 et OpenGL avec le même code. Les deux
backends de fenêtrage Linux sont compilés : l'application fonctionne en session
Wayland comme en session X11, sans variante de binaire.

Les bibliothèques graphiques sont chargées par `dlopen` à l'exécution, pas liées
à la compilation. Conséquence utile : compiler ne demande aucun paquet `-dev`.
Conséquence à connaître : une bibliothèque manquante ne se voit qu'au lancement,
et c'est pour cela que
[packaging/README.md](../packaging/README.md#dépendances-dexécution) les
énumère.

Conséquence assumée aussi : **pas de binaire musl pour l'application**. Un
exécutable statique n'a pas de chargeur dynamique, donc pas de `dlopen`, donc
pas de fenêtre.

### Un runtime tokio gardé en vie

`Backend` possède le `tokio::runtime::Runtime` et le garde vivant aussi
longtemps que l'application. Le détruire fermerait les tâches en cours — flux de
journaux, session de terminal — au moment le plus inattendu. Il est construit une
fois, au démarrage, avec `enable_all()`.

### rustls plutôt qu'OpenSSL

Aucune dépendance système pour TLS : pas de `libssl.so.1.1 not found`, pas de
recompilation à chaque mise à jour d'OpenSSL.

`kube` compile rustls avec `ring`, `reqwest` avec `aws-lc-rs` : deux
fournisseurs cohabitent dans le binaire, et rustls refuse alors de choisir seul.
Le fournisseur doit donc être installé explicitement **au démarrage, avant toute
connexion TLS** — faute de quoi la première requête panique. L'application le
fait au démarrage, dans `app.rs`, avant d'ouvrir la fenêtre.

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
chaque build et échoue au-delà de 60 Mio : il embarque egui, wgpu et le moteur
Kubernetes complet.

### Erreurs typées, traduites une seule fois

Chaque crate définit son `Error` (`thiserror`) avec sa chaîne de causes.
L'application les convertit en `Event::Failed { message }`, affiché en
notification. La logique métier n'a
jamais à savoir où son erreur sera lue.

### `#![forbid(unsafe_code)]`

Dans les quatre crates. Une console d'administration n'a aucune raison de
manipuler de la mémoire non vérifiée.

---

## Ce qui n'est délibérément pas là

| Absent | Pourquoi |
| --- | --- |
| Serveur HTTP, API REST, interface web | Retirés au profit de l'application native : moins de surface d'écoute, aucune authentification à réinventer, aucun jeton à protéger |
| Webview (Tauri, Electron) | Un moteur HTML complet pour afficher des tableaux ; egui rend la même chose avec le GPU et sans dépendance système |
| Base de données | Deux fichiers JSON suffisent ; une base briserait l'autonomie des binaires |
| Comptes utilisateurs | Le contrôle d'accès de Kubernetes, c'est le RBAC ; en dupliquer une version approximative créerait une fausse sécurité |
| Moteur de gabarits Helm | Helm existe et fait autorité ; KubeWatch délègue à son binaire plutôt que d'en produire une imitation divergente |
| Agent dans le cluster | L'API server expose déjà tout ce qui est nécessaire |
| Ordonnanceur intégré | Il vivait dans le serveur ; systemd, cron et le Planificateur de tâches font ce travail mieux et survivent aux redémarrages |
| Binaire musl de l'application | Un exécutable statique ne peut pas `dlopen` Vulkan, Wayland ni X11 : l'artefact serait inutilisable |
| Télémétrie | Aucune donnée ne quitte votre machine, sauf vers votre cluster et vers les services que vous interrogez explicitement |
