# KubeWatch

**Une application de bureau native pour piloter vos clusters Kubernetes.**
KubeWatch lit votre kubeconfig, parle au serveur d'API exactement comme
`kubectl`, et n'ouvre **aucun port réseau**. Une fenêtre, un seul binaire,
aucun agent à déployer dans le cluster.

[![CI](https://github.com/kubewatch-io/kubewatch/actions/workflows/ci.yml/badge.svg)](https://github.com/kubewatch-io/kubewatch/actions/workflows/ci.yml)
[![Release](https://github.com/kubewatch-io/kubewatch/actions/workflows/release.yml/badge.svg)](https://github.com/kubewatch-io/kubewatch/actions/workflows/release.yml)
[![Security](https://github.com/kubewatch-io/kubewatch/actions/workflows/security.yml/badge.svg)](https://github.com/kubewatch-io/kubewatch/actions/workflows/security.yml)
[![Licence Apache-2.0](https://img.shields.io/badge/licence-Apache--2.0-blue.svg)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)

---

## Ce que c'est

| Binaire | Rôle |
| --- | --- |
| `kubewatch-desktop` | L'application graphique : fenêtre native, rendu GPU, huit écrans |

Il s'appuie sur trois bibliothèques du dépôt : `kubewatch-core` (connexion et
ressources Kubernetes), `kubewatch-hub` (registres, charts, catalogue) et
`kubewatch-updater` (détection des mises à jour). Il n'y a pas de ligne de
commande : pour les scripts, `kubectl` reste l'outil.

### Ce qu'il faut savoir avant de commencer

- **Il faut un serveur graphique.** Wayland ou X11 sous Linux, plus un pilote
  Vulkan ou OpenGL fonctionnel. Sur une machine sans affichage, un serveur en
  SSH par exemple, KubeWatch ne démarre pas : c'est un client de poste de
  travail.
- **Le rendu des charts Helm est délégué au binaire `helm`**, s'il est installé
  sur la machine. KubeWatch ne réimplémente pas le moteur de gabarits de Helm.
  Sans `helm`, la recherche de charts fonctionne toujours, mais le rendu échoue
  avec un message expliquant comment l'installer. Le générateur de manifestes
  intégré, lui, ne dépend de rien.
- **Le mode serveur a été retiré.** Il n'y a plus d'interface web, plus d'API
  HTTP, plus de déploiement dans le cluster, plus d'image conteneur. KubeWatch
  est un client de poste de travail, et rien d'autre. Voir le
  [CHANGELOG](CHANGELOG.md#non-publié).
- **Les binaires ne sont pas signés** au sens des systèmes d'exploitation : pas
  de notarisation Apple, pas d'Authenticode. Ils sont en revanche accompagnés
  de sommes SHA-256 signées par cosign, vérifiables (voir
  [docs/installation.md](docs/installation.md#vérifier-lintégrité-et-la-signature)).

---

## Les écrans

Le projet ne publie pas encore de captures d'écran. Plutôt que d'insérer des
images qui n'existent pas, voici ce que chaque écran contient. Le guide complet,
avec la table des raccourcis clavier, est dans
[docs/interface.md](docs/interface.md).

La fenêtre est toujours organisée de la même façon : une **barre supérieure**
(sélecteur de cluster, de namespace, filtre, thème, rafraîchissement), un
**panneau de navigation à gauche** (les huit écrans, puis l'état du cluster
courant), la **zone centrale**, et une **barre d'état** en bas (cluster,
version, requêtes en vol, dernier message).

| Écran | Contenu |
| --- | --- |
| **Vue d'ensemble** | Nœuds et leur état, pods par phase, namespaces, capacités et consommations CPU/mémoire, derniers avertissements du cluster |
| **Ressources** | Un tableau triable et filtrable pour n'importe quel type — y compris les CRD de vos opérateurs — avec un panneau de détail : YAML, évènements, conteneurs, journaux, terminal |
| **Topologie** | Le graphe des objets et de leurs liens — Ingress → Service → Deployment → Pod → nœud, volumes, configuration — déduit des `ownerReferences`, des sélecteurs et des volumes ; zoom, déplacement, deux dispositions, détail au clic |
| **Console YAML** | Un éditeur multi-documents avec coloration syntaxique : `apply` côté serveur, `diff` contre l'état réel, `delete`, en simulation ou pour de vrai |
| **Déployer** | Un assistant en quatre étapes pour mettre n'importe quelle application sur le cluster, avec aperçu de la consommation de ressources avant d'appliquer |
| **Catalogue** | Recherche d'images (Docker Hub, GHCR, Quay, registre OCI générique), tags, inspection, recherche de charts Helm, catalogue d'applications prêtes à déployer, générateur de manifestes |
| **Mises à jour** | Les surveillances, les versions détectées, l'historique des déploiements, l'inventaire des images du cluster, et le bouton qui applique |
| **Réglages** | Clusters enregistrés, import de kubeconfig, connexion distante, thème, zoom, période de rafraîchissement, taille de page, lignes de journal conservées |

Aucune action réseau ne bloque la fenêtre : toutes les opérations partent dans
un fil d'exécution séparé et reviennent sous forme d'évènements. Un cluster
injoignable fait apparaître un message d'erreur, jamais une fenêtre figée.

---

## Fonctionnalités

### Connexion aux clusters

| Mode | Comment |
| --- | --- |
| Kubeconfig standard | Au démarrage (lit `$KUBECONFIG` puis `~/.kube/config`) |
| Fichier `.yml` arbitraire | Bouton « Importer un kubeconfig » de l'écran Réglages |
| Contexte précis | Sélection dans la liste des contextes du kubeconfig |
| API distante (URL + jeton) | Formulaire de l'écran Réglages |
| Certificat client (mTLS) | Champs PEM du formulaire de connexion distante |
| Derrière un mandataire | Champ `proxy-url` du kubeconfig (HTTP ou SOCKS5) |
| Multi-cluster | Plusieurs clusters enregistrés, bascule instantanée depuis la barre supérieure |

Les clusters enregistrés sont persistés dans `clusters.json`, au sein du dossier
d'état (permissions `0600` sous Unix), et rechargés au démarrage suivant.

### Ressources

- Découverte **dynamique** des types : les Custom Resources de vos opérateurs
  (cert-manager, Argo, Flux…) apparaissent au même titre que les Pods.
- Résolution souple des noms : `pods`, `po`, `deploy`, `Deployment`,
  `ingresses.networking.k8s.io`.
- Liste paginée, vue par namespace ou sur tout le cluster, avec sélecteur de
  labels.
- Actions : mise à l'échelle, redémarrage (`rollout restart`), changement
  d'image, retour à la révision précédente, suppression avec politique de
  propagation.
- Évènements récents, globaux ou rattachés à un objet.
- Nœuds : `cordon`, `uncordon`, `drain` (éviction propre), depuis le menu
  contextuel d'un nœud.

### Console YAML

- **Apply côté serveur** (Server-Side Apply, `fieldManager: kubewatch`),
  multi-documents, avec simulation (`dry-run`) et `force` pour reprendre un
  champ détenu par un autre gestionnaire.
- **Diff** entre un manifeste local et l'état réel du cluster, avant d'appliquer.
- **Delete** à partir du même manifeste.
- Le résultat indique, document par document : créé, configuré, inchangé,
  simulé, supprimé ou en échec.

### Journaux et terminal

- Journaux d'un conteneur en flux continu, avec choix du conteneur, nombre de
  lignes, horodatage et conteneur précédent.
- **Terminal interactif** dans un conteneur, avec TTY et redimensionnement
  (`pods/exec`) : `/bin/sh` est proposé et la commande reste modifiable.
- Pas de redirection de port : `kubectl port-forward` reste l'outil pour cela.

### Métriques

Nœuds prêts, pods par état, CPU et mémoire demandés puis réellement consommés,
via `metrics.k8s.io`. Sans metrics-server dans le cluster, ces colonnes
disparaissent proprement : ce n'est pas une erreur, et l'application le dit.

### Hub d'images et de charts

- Recherche d'images sur **Docker Hub**, **GHCR**, **Quay** ou un registre OCI
  générique ; liste des tags, résolution de l'empreinte (digest), inspection
  (ports exposés, variables, labels, entrypoint, architecture).
- Recherche de **charts Helm** (Artifact Hub) et consultation de leurs versions
  et de leurs `values.yaml`.
- **Catalogue intégré** de plus de vingt applications prêtes à déployer : nginx,
  Traefik, PostgreSQL, MySQL, MariaDB, MongoDB, Redis, Memcached, RabbitMQ,
  MinIO, Grafana, Prometheus, Loki, n8n, Gitea, Vaultwarden, Uptime Kuma,
  Jellyfin, Nextcloud, Portainer, Homepage, Adminer, cert-manager,
  metrics-server.
- **Générateur de manifestes** : à partir d'une image et de quelques options
  (ports, variables, ressources, PVC, Service, Ingress), KubeWatch produit un
  YAML multi-documents que vous relisez avant de l'appliquer.
- **Déploiement guidé** : quatre étapes — quoi, comment, vérification,
  résultat. Un profil de taille (*Micro* à *Grand*) remplace les quatre
  quantités Kubernetes, un mode d'accès (interne, port de nœud, IP publique,
  nom de domaine) remplace le type de Service et l'Ingress, et l'étape de
  vérification chiffre ce que le déploiement réservera sur le cluster : part
  occupée, part ajoutée, capacité restante, et les remarques qui évitent un pod
  bloqué au démarrage.

### Détecteur de mises à jour

- Sources : **GitHub Releases**, **registre de conteneurs** (tags OCI),
  **chart Helm**.
- Normalisation des étiquettes : `v1.2.3`, `release-1.2.3`, `1.2`,
  `1.2.3-alpine` sont comprises ; les empreintes, les dates et les `latest` sont
  ignorés.
- Politique par surveillance : canal `major` / `minor` / `patch` /
  `prerelease` / `pinned`, contrainte semver (`>=1.2, <2`), liste d'exclusions,
  fenêtre de maintenance UTC, intervalle de vérification.
- **Déploiement automatique** optionnel : la nouvelle image est posée sur la
  charge de travail et le rollout est suivi. Retour arrière depuis le menu
  contextuel de la charge de travail.
- L'onglet Inventaire liste les images réellement en service et propose les
  surveillances correspondantes.

La vérification se déclenche depuis l'écran Mises à jour, quand l'application
est ouverte. Il n'y a ni démon ni tâche planifiée — voir
[docs/updates.md](docs/updates.md#vérifications-régulières).

---

## Installation

Détail complet, par système : [docs/installation.md](docs/installation.md).

### Linux (x86-64, glibc)

```sh
curl -sSfL https://github.com/kubewatch-io/kubewatch/releases/latest/download/kubewatch-0.1.0-x86_64-unknown-linux-gnu.tar.gz | tar xz
cd kubewatch-0.1.0-x86_64-unknown-linux-gnu
sudo install -m 0755 kubewatch-desktop /usr/local/bin/
sudo install -Dm 0644 io.kubewatch.KubeWatch.desktop /usr/share/applications/io.kubewatch.KubeWatch.desktop
sudo install -Dm 0644 io.kubewatch.KubeWatch.metainfo.xml /usr/share/metainfo/io.kubewatch.KubeWatch.metainfo.xml
kubewatch-desktop
```

### Cibles publiées

| Plateforme | Archive | Contenu |
| --- | --- | --- |
| Linux x86-64 (glibc) | `…-x86_64-unknown-linux-gnu.tar.gz` | `kubewatch-desktop` + métadonnées de bureau |
| Linux ARM64 (glibc) | `…-aarch64-unknown-linux-gnu.tar.gz` | `kubewatch-desktop` + métadonnées de bureau |
| macOS Intel | `…-x86_64-apple-darwin.tar.gz` | `KubeWatch.app` |
| macOS Apple Silicon | `…-aarch64-apple-darwin.tar.gz` | `KubeWatch.app` |
| Windows x86-64 | `…-x86_64-pc-windows-msvc.zip` | `kubewatch-desktop.exe` |

**Pourquoi pas de version musl ?** Parce qu'elle serait
inutilisable. Une application graphique charge Vulkan, OpenGL, Wayland, X11 et
xkbcommon par `dlopen` au démarrage ; un binaire musl statique n'embarque pas le
chargeur dynamique qu'il faut pour cela. Mieux vaut une cible absente qu'un
artefact qui ne s'ouvre pas.

### Depuis les sources

```sh
cargo install --git https://github.com/kubewatch-io/kubewatch --locked kubewatch-desktop
```

La publication sur crates.io est pilotée par la variable de dépôt
`PUBLISH_CRATES` et n'est pas encore activée : utilisez la forme `--git` en
attendant. Il n'existe ni paquet `.deb` ou `.rpm`, ni Flatpak, ni Snap, ni
formule Homebrew — voir [packaging/README.md](packaging/README.md).

---

## Plateformes supportées

| Système | Architectures | Application de bureau | Remarques |
| --- | --- | --- | --- |
| Linux | x86-64, ARM64 | glibc uniquement | Wayland ou X11, plus un pilote Vulkan ou OpenGL |
| macOS | Intel, Apple Silicon | oui | macOS 11 minimum ; bundle non signé, non notarisé |
| Windows | x86-64 | oui | Windows 10 et ultérieur ; pas d'installeur, pas de signature |
| Windows | ARM64 | non | Cible non publiée : elle ne pourrait être compilée que de façon croisée, sans vérification possible |

Versions Kubernetes : 1.24 et ultérieures. La découverte étant dynamique,
KubeWatch s'adapte aux types réellement exposés par votre API server.

---

## Sécurité

**KubeWatch hérite exactement des droits du kubeconfig que vous lui donnez.** Il
n'ajoute aucune restriction : si votre kubeconfig est `cluster-admin`,
l'application l'est aussi.

Trois points, depuis le pivot vers le bureau :

1. **Aucun port n'est ouvert.** Plus de serveur HTTP, donc plus de surface
   d'écoute, plus de jeton d'API à protéger et plus de question d'exposition
   réseau. L'application tourne sous votre identité d'utilisateur.
2. **Les fichiers d'état contiennent des secrets** (jeton GitHub, jetons de
   cluster). Ils sont écrits en `0600` sous Unix, dans un dossier `0700`.
3. **Ce qui sort de la machine** se limite à l'API de votre cluster,
   `api.github.com`, les registres d'images que vous interrogez et
   `artifacthub.io`. Chacun est désactivable en n'utilisant pas la
   fonctionnalité correspondante.

Le modèle de menace complet est dans [docs/security.md](docs/security.md).

---

## Documentation

| Document | Contenu |
| --- | --- |
| [docs/installation.md](docs/installation.md) | Installation par système, prérequis graphiques, vérification, désinstallation |
| [docs/interface.md](docs/interface.md) | Guide de l'interface écran par écran, raccourcis clavier |
| [docs/configuration.md](docs/configuration.md) | Dossier d'état, variables d'environnement, fichiers écrits sur le disque |
| [docs/updates.md](docs/updates.md) | Détecteur de mises à jour, politiques, vérifications régulières |
| [docs/security.md](docs/security.md) | Modèle de menace « application locale », secrets, trafic sortant |
| [docs/architecture.md](docs/architecture.md) | Composants, pont asynchrone, choix techniques |
| [packaging/README.md](packaging/README.md) | Empaquetage sur chaque système, dépendances d'exécution |

---

## Développement

```sh
# Le shim rustup peut être cassé : pointez directement la toolchain
export PATH=~/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH

cargo build --workspace                       # ou : make build
cargo run -p kubewatch-desktop                # ou : make run
cargo watch -w crates -x 'run -p kubewatch-desktop'   # ou : make dev (relance à chaque sauvegarde)
cargo test --workspace --all-features         # ou : make test
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
make ci                                       # reproduit l'essentiel de la CI
```

Structure du workspace :

```
kubewatch/
├── crates/
│   ├── core/      kubewatch-core    — connexion, découverte, ressources, apply,
│   │                                  logs, exec, port-forward, métriques
│   ├── hub/       kubewatch-hub     — registres d'images, charts, catalogue,
│   │                                  générateur de manifestes
│   ├── updater/   kubewatch-updater — GitHub, politiques semver, rollout
│   └── desktop/   kubewatch-desktop — application egui/eframe : pont asynchrone,
│                                      état, vues, widgets
├── packaging/     entrée .desktop, métadonnées AppStream, recettes d'empaquetage
└── docs/          documentation
```

L'interface est écrite en **egui 0.36** en mode immédiat, avec le backend
**wgpu** et les features `wayland` + `x11`. Le cœur asynchrone (`kube`, `tokio`)
tourne dans un fil séparé et communique avec la fenêtre par deux files de
messages : aucune requête réseau ne s'exécute sur le fil d'interface. Le détail
est dans [docs/architecture.md](docs/architecture.md).

---

## Licence

Apache-2.0 — voir [LICENSE](LICENSE).
