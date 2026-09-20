# Changelog

Toutes les modifications notables de KubeWatch sont consignées dans ce fichier.

Le format suit [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/) et le
versioning respecte [SemVer](https://semver.org/lang/fr/).

<!--
Les sections publiées sont régénérées par git-cliff à partir des commits
conventionnels (`git cliff --config cliff.toml --output CHANGELOG.md`).
N'éditez à la main que la section « Non publié », et de préférence pas du tout :
écrivez plutôt des messages de commit corrects.
-->

## [Non publié]

### Changements incompatibles

- **KubeWatch devient une application de bureau native.** Le produit se compose
  désormais d'un seul binaire, `kubewatch-desktop` : un backend Rust Tauri 2 qui
  embarque une interface web (React, TypeScript) construite depuis `ui/`. Le
  mode serveur, la ligne de commande et la première interface native écrite en
  egui ont été retirés.
- **Nouvelles dépendances sous Linux.** La fenêtre est une webview du système :
  webkit2gtk 4.1, javascriptcoregtk 4.1, libsoup 3 et GTK 3 sont nécessaires à
  la compilation comme à l'exécution. Construire depuis les sources demande en
  outre Node.js 22+ et `tauri-cli` 2.
- Le dossier d'état et le format des données sont **inchangés** : les clusters
  enregistrés et les surveillances d'une installation existante sont repris
  tels quels. Le fichier `config.yaml` de la ligne de commande n'est plus lu.

### Retiré

- **La ligne de commande `kubewatch`** (crate `crates/app`) : ses sous-commandes,
  la sortie `json` / `yaml`, la complétion du shell, `self-update`,
  `port-forward`, les options et variables `KUBEWATCH_*` qu'elle lisait et le
  fichier `config.yaml`. KubeWatch est une application de bureau, et rien
  d'autre ; pour les scripts, `kubectl` reste l'outil. Conséquence directe :
  plus aucune vérification de mises à jour ne peut être planifiée par systemd
  ou cron, elle part du bouton de l'écran Mises à jour.
- **Serveur HTTP et interface web** : le serveur `axum`, ses routes `/api`, ses
  deux WebSockets (journaux et terminal) et l'interface web embarquée
  (`ui/`, `rust-embed`). La sous-commande `kubewatch serve` n'existe plus.
- **Déploiement dans le cluster** : manifestes Kubernetes, chart Helm,
  `docker-compose.yml`, `Dockerfile`, image `ghcr.io/kubewatch-io/kubewatch` et
  le workflow de publication d'image.
- **Webhook GitHub** : plus aucun binaire n'expose d'endpoint HTTP pour le
  recevoir. Le module de vérification HMAC-SHA256 reste dans la bibliothèque
  `kubewatch-updater`, mais n'est plus appelé. Supprimez le webhook déclaré
  côté GitHub, ses livraisons partent dans le vide.
- **Ordonnanceur intégré** : il tournait dans le serveur. Les vérifications de
  mises à jour se déclenchent à la demande, depuis l'écran Mises à jour — voir
  [docs/updates.md](docs/updates.md#vérifications-régulières).
- **`docs/api.md`** : sans API HTTP, la référence n'a plus d'objet.
- Les artefacts de release **musl** et **Windows ARM64** : une application qui
  charge Vulkan, Wayland et X11 par `dlopen` ne peut pas être un exécutable
  statique, et une cible graphique compilée de façon croisée ne peut pas être
  vérifiée. Ces cibles ne servaient que la ligne de commande.

### Nouveautés

- **Interface entièrement nouvelle** (`ui/`, React 19 + TypeScript + Vite),
  servie par le backend Tauri : barre latérale, barre de cluster et de
  namespace, recherche globale (Ctrl+K), thème clair/sombre ou système,
  raccourcis Ctrl+1…7, Ctrl+, et Ctrl+R. Les écrans : vue d'ensemble, clusters
  (import de kubeconfig par contexte, connexion distante), ressources (table par
  type, panneau de détail avec YAML éditable, évènements, journaux en flux,
  terminal, actions : redimensionner, redémarrer, retour arrière, image,
  cordon/drain, suppression), console YAML (simulation, différences,
  application, suppression), topologie, hub, déploiement, mises à jour,
  réglages. `make run` lance le tout avec rechargement à chaud ;
  `make bundles` produit AppImage, `.deb` et `.rpm`.
- **Test de fumée des écrans** : `make ui-test` monte chacun d'eux dans un DOM
  simulé sur un instantané de cluster anonymisé, et échoue si l'un lève une
  erreur au rendu. Il attrape ce que le vérificateur de types ne voit pas : les
  écarts entre les types déclarés et le JSON réellement produit par le backend.
- **Assistant IA** (`crates/ai`, panneau Ctrl+J dans l'application Tauri) :
  dialogue en flux avec **Claude** (API Anthropic), **ChatGPT** (API OpenAI)
  ou **un modèle local** via tout serveur compatible OpenAI (LM Studio, Ollama,
  llama.cpp, Jan…). Plusieurs profils, liste des modèles du fournisseur, clés
  conservées dans `ai.json` (0600) et jamais renvoyées à l'interface. L'assistant
  reçoit le contexte de l'écran (cluster, namespace, objet sélectionné) et
  dispose d'**outils en lecture seule** sur le cluster — synthèse, namespaces,
  listes d'objets, YAML, évènements, journaux, métriques — qu'il enchaîne
  lui-même pour diagnostiquer ; il ne modifie jamais rien : les YAML qu'il
  propose s'ouvrent d'un clic dans la console YAML. Refus et troncatures du
  fournisseur sont affichés tels quels.
- **`make appimage`** produit un AppImage autonome pour Linux
  (`dist/KubeWatch-<version>-<arch>.AppImage`, avec son `.sha256`). Il
  n'embarque aucune bibliothèque graphique ni aucune information de mise à
  jour : le système hôte fournit Vulkan, Wayland, X11 et xkbcommon, comme pour
  le binaire nu.
- **Une icône d'application est livrée** (`packaging/icons/hicolor/`, SVG et
  PNG 256 px), reprise du dessin que la fenêtre affiche déjà. Les archives
  Linux l'embarquent et `make desktop-install` l'installe dans le thème
  d'icônes de l'utilisateur.
- **Écran « Topologie » (`Ctrl+3`)** : le graphe des objets du cluster et de
  leurs liens — Ingress → Service → Deployment / StatefulSet / DaemonSet /
  CronJob → Pod → nœud, volumes persistants, ConfigMaps et Secrets. Les liens
  viennent des `ownerReferences`, des sélecteurs de services confrontés aux
  labels, des backends d'ingress, du `nodeName` et des volumes de chaque pod.
  Deux dispositions (par couches, ou organique par forces), zoom et
  déplacement de la vue, nœuds déplaçables à la main, couches activables
  (ReplicaSets et configuration masqués par défaut), mise en avant du
  voisinage au survol, filtre de la barre supérieure, ouverture du panneau de
  détail au clic, menu contextuel vers l'écran Ressources. Un objet référencé
  mais introuvable (backend d'ingress absent, ConfigMap manquante) apparaît
  comme nœud « non listé » : c'est le genre d'erreur que l'écran sert à voir.
  Côté cœur, `ObjectSummary` porte désormais les propriétaires (`owners`), et
  les résumés de pods et d'ingress exposent volumes, ConfigMaps, Secrets et
  backends.
- **Icônes Phosphor** : l'interface utilise le jeu d'icônes
  [Phosphor](https://phosphoricons.com/) (paquet `@phosphor-icons/react`,
  licence MIT). Navigation, bandeaux de notification, boutons de fermeture, tri
  des colonnes, verdicts et avertissements portent des pictogrammes dessinés
  d'un même trait plutôt que des glyphes Unicode inégaux d'une police à l'autre.
- **Écran « Déployer » (`Ctrl+5`)** : un assistant en quatre étapes — choisir
  l'application (catalogue embarqué ou image quelconque, dont les ports et les
  variables sont lus dans le registre), la régler (profil de taille, mode
  d'accès, stockage, variables), **vérifier**, appliquer. L'étape de
  vérification montre ce qui sera créé, le manifeste, et un **aperçu de la
  consommation** : pour le processeur et la mémoire, la part déjà occupée du
  cluster et celle que le déploiement réserve, avec un verdict (tient / charge
  le cluster / dépasse la capacité) et les remarques qui comptent — limite
  inférieure à la demande, volume `ReadWriteOnce` partagé entre répliques, tag
  mouvant, demande de ressources absente. Le formulaire complet du catalogue
  reste accessible, et les deux écrans se passent le déploiement en cours dans
  les deux sens.
- **Module `kubewatch-hub::sizing`** : profils de taille (*Micro*, *Petit*,
  *Moyen*, *Grand*), calcul de l'empreinte d'un déploiement (demandes et
  limites multipliées par les répliques, stockage) et confrontation à la
  capacité du cluster. Logique pure, sans interface ni réseau.
- **Crate `crates/desktop`** : pont asynchrone (`Command` / `Event` /
  `RequestId`) entre l'interface synchrone en mode immédiat et le cœur
  asynchrone `tokio` + `kube`, huit écrans (vue d'ensemble, ressources,
  topologie, console YAML, déploiement guidé, catalogue, mises à jour,
  réglages), panneau de détail
  avec YAML, évènements, conteneurs, journaux en direct et terminal interactif.
- **Métadonnées de bureau** : `packaging/io.kubewatch.KubeWatch.desktop` (entrée freedesktop)
  et `packaging/io.kubewatch.KubeWatch.metainfo.xml` (AppStream), validées à
  chaque exécution de la CI.
- **`packaging/README.md`** : recettes d'empaquetage par système, dépendances
  d'exécution par distribution, et ce qui n'existe pas encore.
- **`docs/interface.md`** : guide de l'interface, écran par écran, et table des
  raccourcis clavier.
- **Bundle `.app` minimal sur macOS**, assemblé par la chaîne de release. Il
  n'est **ni signé ni notarisé** : Gatekeeper demandera de retirer l'attribut de
  quarantaine.
- CI : installation des dépendances système graphiques sur Linux, compilation
  du binaire sur les trois systèmes, budget de taille, et validation des
  métadonnées freedesktop.

### Déprécié

- Les clés de configuration `listen`, `apiToken`, `noBrowser` et `scheduler`,
  ainsi que les variables `KUBEWATCH_LISTEN`, `KUBEWATCH_ADDR`,
  `KUBEWATCH_API_TOKEN`, `KUBEWATCH_NO_BROWSER`, `KUBEWATCH_SCHEDULER` et
  l'option globale `--api-token`, sont encore acceptées **sans effet** : le
  composant qu'elles réglaient n'existe plus. Elles seront retirées à la
  prochaine version majeure.

## [0.1.0] — 2026-09-18

Première version publique : un binaire unique, sans dépendance système, pour
piloter un ou plusieurs clusters Kubernetes.

### Nouveautés

#### Connexion aux clusters

- Quatre modes de connexion : kubeconfig local, fichier `.yml` fourni
  explicitement (ou collé dans l'interface), connexion distante par
  URL + jeton (avec CA, certificat client et proxy optionnels), et mode
  in-cluster via le ServiceAccount monté.
- Gestion de plusieurs clusters en parallèle, import de tous les contextes d'un
  kubeconfig, sélection du cluster courant et persistance locale des connexions.
- Découverte dynamique des ressources de l'API : les CRD du cluster sont
  listées et manipulables au même titre que les types intégrés.

#### Exploration et pilotage

- Vue d'ensemble du cluster : nœuds, pods par état, namespaces, capacités et
  consommations CPU/mémoire, charges de travail et avertissements.
- Liste paginée de n'importe quel type de ressource, avec filtres par
  namespace, sélecteur de labels et sélecteur de champs.
- Lecture des objets en JSON et en YAML, remplacement par YAML, suppression
  avec politique de propagation.
- Actions sur les charges de travail : mise à l'échelle, redémarrage,
  changement d'image, retour arrière.
- Actions sur les nœuds : `cordon`/`uncordon` et `drain`.
- Journal des évènements récents du cluster, filtrable par namespace ou par
  objet impliqué.
- Métriques des nœuds et des pods lorsque `metrics-server` est disponible, avec
  dégradation propre lorsqu'il ne l'est pas.

#### Fichiers YAML

- `apply` multi-documents en server-side apply, avec `dry-run`, `force` et
  namespace par défaut.
- `diff` entre le manifeste local et l'état du cluster.
- Suppression pilotée par un manifeste (`delete -f`).

#### Logs, terminal et port-forward

- Consultation des logs d'un conteneur (instantané ou flux temps réel par
  WebSocket), avec horodatage, `tail` et logs du conteneur précédent.
- Terminal interactif dans un conteneur (`exec`) avec TTY et redimensionnement.
- Redirection de port locale vers un pod.

#### Hub d'images et déploiement

- Recherche d'images sur Docker Hub, GHCR et Quay, consultation des tags,
  résolution des digests et inspection des métadonnées d'image.
- Recherche de charts Helm et rendu local lorsque le binaire `helm` est présent.
- Catalogue d'applications prêtes à déployer (bases de données, reverse
  proxies, supervision, stockage objet, outils d'auto-hébergement…).
- Générateur de manifestes : Deployment, Service, Ingress et PVC produits à
  partir d'un formulaire, prévisualisables en YAML avant application.

#### Détection d'updates et CI/CD

- Watchers déclarant une source d'update (release GitHub, registre de
  conteneurs, chart Helm) pour une charge de travail donnée.
- Politiques de mise à jour par canal SemVer (majeure, mineure, correctif,
  préversion, épinglée), avec contrainte de version, liste d'exclusions,
  fenêtre de maintenance et application automatique optionnelle.
- Scan du cluster pour découvrir les images en cours d'exécution et proposer
  les watchers correspondants.
- Application d'une update détectée avec suivi du rollout, retour arrière et
  historique des déploiements.
- Endpoint de webhook GitHub avec vérification HMAC-SHA256 à temps constant.
- Vérification de la disponibilité d'une nouvelle version de KubeWatch
  lui-même.

#### Interface et distribution

- Interface web embarquée dans le binaire : aucun fichier à déployer à côté.
- API HTTP complète sous `/api`, utilisable directement pour l'automatisation.
- CLI pour les opérations courantes sans passer par le navigateur.
- Binaires pour Linux (glibc et musl), macOS et Windows, en x86_64 et aarch64.
- Image conteneur multi-arch `ghcr.io/kubewatch-io/kubewatch`, basée sur
  distroless, exécutée en utilisateur non privilégié.

### Sécurité

- Sommes de contrôle SHA-256 publiées et signées sans clé (Sigstore cosign,
  identité OIDC du workflow de release).
- SBOM et attestation de provenance générés pour chaque image conteneur.
- Audit continu des dépendances : RustSec, `cargo-deny` (licences, sources,
  interdictions) et analyse CodeQL des workflows.
- Liste blanche stricte de licences : aucune dépendance sous copyleft fort.
- Fichier d'état local (jetons GitHub, identifiants de registre) écrit de
  façon atomique et restreint au propriétaire (mode `0600` sous Unix).

[Non publié]: https://github.com/kubewatch-io/kubewatch/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/kubewatch-io/kubewatch/releases/tag/v0.1.0
