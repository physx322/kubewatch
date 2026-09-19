# Installation

KubeWatch est **un exécutable autonome**, `kubewatch-desktop`. Il n'y a ni
service à enregistrer, ni base de données à provisionner, ni interpréteur à
installer. Vous copiez le fichier dans votre `PATH`, et vous le lancez.

- [Prérequis](#prérequis)
- [Binaire précompilé](#binaire-précompilé)
- [Vérifier l'intégrité et la signature](#vérifier-lintégrité-et-la-signature)
- [Premier lancement](#premier-lancement)
- [Depuis les sources](#depuis-les-sources)
- [Mise à jour](#mise-à-jour)
- [Désinstallation](#désinstallation)
- [Diagnostic](#diagnostic)

---

## Prérequis

| Élément | Nécessaire ? | Détail |
| --- | --- | --- |
| Un kubeconfig | Oui, sauf connexion distante par URL + jeton | `~/.kube/config`, `$KUBECONFIG`, ou n'importe quel `.yml` que vous désignez |
| Kubernetes 1.24+ | Oui | La découverte des types est dynamique, aucune version n'est codée en dur |
| Un serveur graphique | Oui | Wayland ou X11 sous Linux ; natif sous macOS et Windows |
| Un pilote Vulkan ou OpenGL | Oui | Mesa suffit, y compris en rendu logiciel |
| `kubectl` | Non | KubeWatch parle directement au serveur d'API |
| `helm` | Seulement pour le rendu de charts | Cherché dans le `PATH`, ou désigné par `KUBEWATCH_HELM_BIN` |
| metrics-server | Seulement pour les métriques | Sans lui, les colonnes CPU/mémoire disparaissent, sans erreur |

Sur une machine sans affichage, un serveur en SSH par exemple, KubeWatch ne
démarre pas : c'est un client de poste de travail, sans mode texte. Pour les
scripts et les pipelines, `kubectl` reste l'outil.

### Bibliothèques d'exécution, sous Linux

L'application charge ses bibliothèques graphiques à l'exécution, par `dlopen` —
elles ne sont donc pas vérifiées à l'installation, et leur absence ne se
manifeste qu'au lancement, par une fenêtre qui ne s'ouvre pas.

Sur un poste de travail équipé d'un bureau, tout est déjà là. Sur une machine
minimale, il faut au minimum :

```sh
# Debian / Ubuntu
sudo apt-get install libwayland-client0 libxkbcommon0 libxkbcommon-x11-0 \
  libx11-6 libxcursor1 libxrandr2 libxi6 libvulkan1 mesa-vulkan-drivers \
  xdg-desktop-portal xdg-desktop-portal-gtk

# Fedora
sudo dnf install wayland-libs-client libxkbcommon libxkbcommon-x11 libX11 \
  libXcursor libXrandr libXi vulkan-loader mesa-vulkan-drivers \
  xdg-desktop-portal xdg-desktop-portal-gtk

# Arch
sudo pacman -S wayland libxkbcommon libxkbcommon-x11 libx11 libxcursor \
  libxrandr libxi vulkan-icd-loader vulkan-radeon xdg-desktop-portal \
  xdg-desktop-portal-gtk
```

Le portail XDG (`xdg-desktop-portal` et l'implémentation de votre bureau) ne
sert qu'aux sélecteurs de fichiers — « ouvrir un kubeconfig », « enregistrer un
YAML ». Sans lui, le reste de l'application fonctionne, mais ces boîtes de
dialogue n'apparaissent pas. La liste complète, par distribution, est dans
[packaging/README.md](../packaging/README.md#dépendances-dexécution).

---

## Binaire précompilé

C'est la méthode recommandée : rien à compiler.

### Cibles disponibles

| Plateforme | Archive | Contenu |
| --- | --- | --- |
| Linux x86-64 (glibc) | `kubewatch-<version>-x86_64-unknown-linux-gnu.tar.gz` | `kubewatch-desktop` + métadonnées de bureau |
| Linux ARM64 (glibc) | `kubewatch-<version>-aarch64-unknown-linux-gnu.tar.gz` | `kubewatch-desktop` + métadonnées de bureau |
| macOS Intel | `kubewatch-<version>-x86_64-apple-darwin.tar.gz` | `KubeWatch.app` |
| macOS Apple Silicon | `kubewatch-<version>-aarch64-apple-darwin.tar.gz` | `KubeWatch.app` |
| Windows x86-64 | `kubewatch-<version>-x86_64-pc-windows-msvc.zip` | `kubewatch-desktop.exe` |

**Pourquoi pas de version musl.** Une application graphique
ouvre Vulkan, OpenGL, Wayland, X11 et xkbcommon par `dlopen` au démarrage. Un
binaire musl statique n'embarque pas le chargeur dynamique nécessaire : il
démarrerait, puis échouerait à créer une fenêtre. Plutôt que de publier un
artefact cassé, la cible est absente — et vous le savez avant de la chercher.

**Pourquoi ARM64 Windows n'est pas publié.** La cible ne pourrait être compilée
que de façon croisée depuis un runner x86-64, où rien ne permet de vérifier
qu'une fenêtre s'ouvre. On ne publie pas un binaire graphique que personne n'a
pu lancer.

**Version de glibc.** Les binaires Linux glibc sont compilés sur Ubuntu 22.04 :
ils exigent la **glibc 2.35 ou plus récente**. Sur une distribution plus
ancienne, compilez l'application vous-même (voir
[Depuis les sources](#depuis-les-sources)).

### Linux

```sh
VERSION=0.1.0
curl -sSfLO "https://github.com/kubewatch-io/kubewatch/releases/download/v${VERSION}/kubewatch-${VERSION}-x86_64-unknown-linux-gnu.tar.gz"
tar xzf "kubewatch-${VERSION}-x86_64-unknown-linux-gnu.tar.gz"
cd "kubewatch-${VERSION}-x86_64-unknown-linux-gnu"

# Le binaire
sudo install -m 0755 kubewatch-desktop /usr/local/bin/

# L'entrée de menu et les métadonnées de logithèque
sudo install -Dm 0644 io.kubewatch.KubeWatch.desktop \
  /usr/share/applications/io.kubewatch.KubeWatch.desktop
sudo install -Dm 0644 io.kubewatch.KubeWatch.metainfo.xml \
  /usr/share/metainfo/io.kubewatch.KubeWatch.metainfo.xml
sudo update-desktop-database /usr/share/applications || true

kubewatch-desktop
```

Aucune icône n'est livrée : l'entrée de menu s'affiche avec un pictogramme
générique. Voir [packaging/README.md](../packaging/README.md#icône) pour en
ajouter une.

### macOS

```sh
VERSION=0.1.0
ARCH=aarch64   # ou x86_64 sur un Mac Intel
curl -sSfLO "https://github.com/kubewatch-io/kubewatch/releases/download/v${VERSION}/kubewatch-${VERSION}-${ARCH}-apple-darwin.tar.gz"
tar xzf "kubewatch-${VERSION}-${ARCH}-apple-darwin.tar.gz"
cd "kubewatch-${VERSION}-${ARCH}-apple-darwin"

sudo install -m 0755 kubewatch /usr/local/bin/
cp -R KubeWatch.app /Applications/
```

### Windows (PowerShell)

```powershell
$version = "0.1.0"
$url = "https://github.com/kubewatch-io/kubewatch/releases/download/v$version/kubewatch-$version-x86_64-pc-windows-msvc.zip"
Invoke-WebRequest -Uri $url -OutFile kubewatch.zip
Expand-Archive kubewatch.zip -DestinationPath .

$dest = "$env:LOCALAPPDATA\Programs\KubeWatch"
New-Item -ItemType Directory -Force -Path $dest | Out-Null
Copy-Item ".\kubewatch-$version-x86_64-pc-windows-msvc\*.exe" $dest

# Ajouter le dossier au PATH de l'utilisateur
[Environment]::SetEnvironmentVariable(
  "Path",
  [Environment]::GetEnvironmentVariable("Path", "User") + ";$dest",
  "User")
```

---

## Vérifier l'intégrité et la signature

Chaque release publie `SHA256SUMS`, sa signature `SHA256SUMS.sig` et le
certificat éphémère `SHA256SUMS.pem`. La signature est **keyless** : elle est
produite par le workflow de release lui-même, dont l'identité OIDC est attestée
par Sigstore. Aucune clé privée n'existe, donc aucune ne peut fuiter.

```sh
VERSION=0.1.0
BASE="https://github.com/kubewatch-io/kubewatch/releases/download/v${VERSION}"
curl -sSfLO "${BASE}/SHA256SUMS"
curl -sSfLO "${BASE}/SHA256SUMS.sig"
curl -sSfLO "${BASE}/SHA256SUMS.pem"

# 1. Intégrité
sha256sum -c SHA256SUMS --ignore-missing

# 2. Authenticité (cosign : https://docs.sigstore.dev)
cosign verify-blob SHA256SUMS \
  --signature SHA256SUMS.sig \
  --certificate SHA256SUMS.pem \
  --certificate-identity-regexp "^https://github.com/kubewatch-io/kubewatch/" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

Une somme qui ne correspond pas, ou une signature refusée : n'installez pas, et
ouvrez un ticket.

**Ce que cette vérification ne fait pas.** Elle prouve que l'archive vient bien
de ce workflow de release. Elle ne remplace pas une signature système : les
binaires ne sont **ni notarisés par Apple, ni signés en Authenticode**, et les
deux systèmes vous le diront à leur manière (voir ci-dessous).

---

## Premier lancement

### Linux

```sh
kubewatch-desktop
```

L'application lit `$KUBECONFIG`, puis `~/.kube/config`. Si aucun cluster n'est
joignable, la fenêtre s'ouvre quand même : vous pouvez importer un kubeconfig ou
déclarer une connexion distante depuis l'écran Réglages.

### macOS : quarantaine Gatekeeper

Le bundle n'est ni signé ni notarisé — le projet ne dispose d'aucun certificat
Apple Developer. Au premier lancement, macOS refusera de l'ouvrir. Après avoir
vérifié la somme SHA-256 et la signature cosign :

```sh
xattr -dr com.apple.quarantine /Applications/KubeWatch.app
xattr -d com.apple.quarantine /usr/local/bin/kubewatch
```

C'est une manipulation à faire en connaissance de cause : elle désactive une
vérification de sécurité réelle. Faites-la après la vérification, jamais avant.

### Windows : SmartScreen

Les exécutables ne sont pas signés en Authenticode. SmartScreen affichera
« Windows a protégé votre ordinateur » au premier lancement :
*Informations complémentaires* → *Exécuter quand même*, après avoir vérifié la
somme de contrôle.

`kubewatch-desktop.exe` est compilé sans console : lancé depuis l'Explorateur,
il n'ouvre pas de fenêtre noire derrière lui, mais il n'écrit rien non plus sur
la sortie standard. Pour voir ses traces, lancez-le depuis un terminal avec
`RUST_LOG=debug`.

---

## Depuis les sources

### Dépendances de compilation

Rust 1.85 ou plus récent. Sous Linux, **aucune bibliothèque de développement
n'est requise** avec la configuration actuelle du dépôt : les bibliothèques
graphiques sont chargées à l'exécution, pas liées à la compilation. Ni GTK, ni
Qt, ni webkit2gtk — KubeWatch n'embarque pas de webview.

Si vous préférez la ceinture et les bretelles (et c'est ce que fait la CI) :

```sh
sudo apt-get install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libx11-dev libxcursor-dev libxrandr-dev libxi-dev \
  libgl1-mesa-dev libegl1-mesa-dev
```

Le raisonnement complet, avec la façon de le vérifier soi-même, est dans
[packaging/README.md](../packaging/README.md#dépendances-de-compilation).

### Compiler et installer

```sh
# Depuis le dépôt distant
cargo install --git https://github.com/kubewatch-io/kubewatch --locked kubewatch-desktop

# Depuis un clone local
git clone https://github.com/kubewatch-io/kubewatch
cd kubewatch
make release            # le binaire, profil dist
make install            # dans ~/.cargo/bin
make desktop-install    # l'entrée de menu, pour l'utilisateur courant
```

La publication sur crates.io n'est pas encore activée (variable de dépôt
`PUBLISH_CRATES`) : `cargo install kubewatch-desktop` ne fonctionnera qu'ensuite.

### Compilation croisée

```sh
make release TARGET=aarch64-unknown-linux-gnu
```

Pour l'application de bureau, préférez une compilation **native** sur la
machine cible, ou un conteneur de la même architecture. Les images `cross` ne
contiennent pas d'environnement graphique ; la compilation peut aboutir, mais
rien n'y vérifie le résultat. La chaîne de release, elle, compile chaque cible
graphique sur un runner de son architecture.

---

## Mise à jour

L'application ne vérifie pas sa propre version et ne se met jamais à jour
toute seule. Suivez la page des releases du dépôt ; le remplacement consiste à
réinstaller l'archive de la nouvelle version, exactement comme la première fois
— c'est aussi l'occasion de revérifier la signature.

---

## Désinstallation

```sh
# Binaires
sudo rm -f /usr/local/bin/kubewatch /usr/local/bin/kubewatch-desktop
# ou, si installés par cargo :
cargo uninstall kubewatch kubewatch-desktop

# Entrée de menu et métadonnées
sudo rm -f /usr/share/applications/io.kubewatch.KubeWatch.desktop \
           /usr/share/metainfo/io.kubewatch.KubeWatch.metainfo.xml
# ou, pour une installation par utilisateur :
make desktop-uninstall

# État local : clusters enregistrés, surveillances, historique, jetons
rm -rf ~/.local/share/kubewatch      # Linux
rm -rf ~/Library/Application\ Support/kubewatch   # macOS
# Windows : %APPDATA%\kubewatch

# Préférences de l'interface (thème, zoom, géométrie de la fenêtre)
rm -rf ~/.local/share/KubeWatch      # Linux
rm -rf ~/Library/Application\ Support/KubeWatch   # macOS
# Windows : %APPDATA%\KubeWatch

# Ancien fichier de configuration de la ligne de commande, s'il existe encore
rm -f ~/.config/kubewatch/config.yaml
```

Le dossier d'état contient des secrets (jetons GitHub, jetons de cluster) :
supprimez-le si vous désinstallez pour de bon.

---

## Diagnostic

### Symptômes fréquents

| Symptôme | Piste |
| --- | --- |
| Colonnes CPU/mémoire vides | metrics-server absent : `kubectl top nodes` échoue aussi |
| `403` sur une action | Droits du kubeconfig insuffisants |
| `le binaire « helm » est introuvable` | Installez `helm`, ou utilisez le générateur de manifestes intégré |
| `429` / `rateLimited` sur le catalogue ou les mises à jour | Renseignez un jeton GitHub dans l'onglet Réglages de l'écran Mises à jour |

### L'application de bureau

Lancez-la depuis un terminal pour voir ce qu'elle raconte :

```sh
RUST_LOG=debug kubewatch-desktop
```

| Symptôme | Piste |
| --- | --- |
| `neither WAYLAND_DISPLAY nor WAYLAND_SOCKET nor DISPLAY is set` | Aucune session graphique : vous êtes en SSH ou en console |
| Aucune fenêtre, erreur d'adaptateur wgpu | Aucun pilote Vulkan : installez `mesa-vulkan-drivers`, ou forcez OpenGL avec `WGPU_BACKEND=gl` |
| Fenêtre noire, ou rendu incohérent | Essayez l'autre backend : `WGPU_BACKEND=vulkan` ou `WGPU_BACKEND=gl` |
| Deux cartes graphiques, mauvaise sélection | `WGPU_POWER_PREF=low` (économie) ou `high` (performance) |
| La fenêtre ne se rattache pas à l'icône du lanceur | `StartupWMClass` du `.desktop` ne correspond pas à l'`app_id` réel — voir [packaging/README.md](../packaging/README.md#construire-un-paquet-de-distribution) |
| « Ouvrir un fichier » ne fait rien | `xdg-desktop-portal` absent ou non démarré |
| Machine virtuelle ou WSL sans GPU | Installez le rendu logiciel (`mesa-vulkan-drivers` fournit lavapipe), ou `WGPU_BACKEND=gl` avec `LIBGL_ALWAYS_SOFTWARE=1` |

Ces variables (`WGPU_BACKEND`, `WGPU_POWER_PREF`) sont lues par wgpu lui-même,
pas par KubeWatch : elles fonctionnent avec la configuration par défaut de
l'application. Valeurs acceptées par `WGPU_BACKEND` : `vulkan`, `gl`, `dx12`,
`metal`.

Voir aussi [configuration.md](configuration.md), [interface.md](interface.md) et
[security.md](security.md).
