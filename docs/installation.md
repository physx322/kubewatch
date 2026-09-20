# Installation

KubeWatch est **un exécutable autonome**, `kubewatch-desktop`. Il n'y a ni
service à enregistrer, ni base de données à provisionner, ni interpréteur à
installer : l'interface est compilée d'avance et embarquée dans le binaire.
Vous copiez le fichier dans votre `PATH`, et vous le lancez.

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
| Une session graphique | Oui | Wayland ou X11 sous Linux ; natif sous macOS et Windows |
| La webview du système | Oui | WebKitGTK 4.1 et GTK 3 sous Linux (voir ci-dessous) ; fournie par le système sous macOS et Windows |
| `kubectl` | Non | KubeWatch parle directement au serveur d'API |
| `helm` | Non | Le rendu de charts n'est pas exposé par l'application |
| metrics-server | Seulement pour les métriques | Sans lui, les jauges CPU/mémoire et la prévision de capacité disparaissent, sans erreur |

Sur une machine sans affichage, un serveur en SSH par exemple, KubeWatch ne
démarre pas : c'est un client de poste de travail, sans mode texte. Pour les
scripts et les pipelines, `kubectl` reste l'outil.

### Bibliothèques d'exécution, sous Linux

Contrairement aux versions précédentes, ces bibliothèques ne sont **pas**
chargées à la demande : elles sont **liées à l'édition de liens**. Leur absence
ne se manifeste donc pas par une fenêtre qui ne s'ouvre pas, mais par un refus
du chargeur dynamique, avec le nom du fichier manquant. C'est plus brutal, et
plus clair.

La liste exacte se lit sur le binaire lui-même :

```sh
readelf -d kubewatch-desktop | grep NEEDED
```

Sur un poste de travail équipé d'un bureau GTK, tout est déjà là. Sur une
machine minimale :

```sh
# Debian / Ubuntu  (sur Ubuntu 24.04 et Debian 13, certains de ces paquets
# portent le suffixe « t64 » : libgtk-3-0t64, libglib2.0-0t64…)
sudo apt-get install libwebkit2gtk-4.1-0 libjavascriptcoregtk-4.1-0 \
  libsoup-3.0-0 libgtk-3-0 libgdk-pixbuf-2.0-0 libcairo2 libglib2.0-0 \
  libdbus-1-3 xdg-desktop-portal xdg-desktop-portal-gtk

# Fedora
sudo dnf install webkit2gtk4.1 libsoup3 gtk3 gdk-pixbuf2 cairo glib2 \
  dbus-libs xdg-desktop-portal xdg-desktop-portal-gtk

# Arch
sudo pacman -S webkit2gtk-4.1 libsoup3 gtk3 gdk-pixbuf2 cairo glib2 dbus \
  xdg-desktop-portal xdg-desktop-portal-gtk
```

Le portail XDG (`xdg-desktop-portal` et l'implémentation de votre bureau) ne
sert qu'au sélecteur de fichiers — « Parcourir… » pour choisir un kubeconfig.
Sans lui, le reste de l'application fonctionne, mais cette boîte de dialogue
n'apparaît pas ; le chemin peut toujours être saisi à la main. La table
complète des bibliothèques liées est dans
[packaging/README.md](../packaging/README.md#dépendances-dexécution).

---

## Binaire précompilé

C'est la méthode recommandée : rien à compiler.

### Cibles disponibles

| Plateforme | Archive | Contenu |
| --- | --- | --- |
| Linux x86-64 (glibc) | `kubewatch-<version>-x86_64-unknown-linux-gnu.tar.gz` | `kubewatch-desktop`, métadonnées de bureau, icône |
| Linux ARM64 (glibc) | `kubewatch-<version>-aarch64-unknown-linux-gnu.tar.gz` | `kubewatch-desktop`, métadonnées de bureau, icône |
| macOS Intel | `kubewatch-<version>-x86_64-apple-darwin.tar.gz` | `KubeWatch.app` |
| macOS Apple Silicon | `kubewatch-<version>-aarch64-apple-darwin.tar.gz` | `KubeWatch.app` |
| Windows x86-64 | `kubewatch-<version>-x86_64-pc-windows-msvc.zip` | `kubewatch-desktop.exe` |

Toutes contiennent aussi `LICENSE`, `README.md` et `CHANGELOG.md`. Les archives
Linux ajoutent `io.kubewatch.KubeWatch.desktop`,
`io.kubewatch.KubeWatch.metainfo.xml`, le dossier `icons/` et une copie de
`packaging/README.md` sous le nom `INSTALL-packaging.md`.

**Pourquoi pas de version musl.** L'interface s'affiche dans la webview du
système : sous Linux, le binaire est lié à WebKitGTK, GTK 3, libsoup 3 et GLib,
qui sont des bibliothèques partagées. Un exécutable statique ne peut pas les
emporter, et n'aurait rien à afficher. Plutôt que de publier un artefact cassé,
la cible est absente — et vous le savez avant de la chercher.

**Pourquoi ARM64 Windows n'est pas publié.** La cible ne pourrait être compilée
que de façon croisée depuis un runner x86-64, où rien ne permet de vérifier
qu'une fenêtre s'ouvre. On ne publie pas un binaire graphique que personne n'a
pu lancer.

**Version de glibc, version de WebKitGTK.** Les binaires Linux glibc sont
compilés sur Ubuntu 22.04 : ils exigent la **glibc 2.35 ou plus récente**, et
une WebKitGTK **4.1** (soit `libwebkit2gtk-4.1.so.0`, pas la série 4.0). Sur une
distribution plus ancienne, compilez l'application vous-même (voir
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

# L'icône
sudo cp -r icons/hicolor /usr/share/icons/

sudo update-desktop-database /usr/share/applications || true
sudo gtk-update-icon-cache /usr/share/icons/hicolor || true

kubewatch-desktop
```

Pour une installation sans `sudo`, les mêmes fichiers vont dans
`~/.local/bin`, `~/.local/share/applications`, `~/.local/share/metainfo` et
`~/.local/share/icons` ; depuis un clone du dépôt, `make desktop-install` s'en
charge. Voir [packaging/README.md](../packaging/README.md#icône).

### macOS

```sh
VERSION=0.1.0
ARCH=aarch64   # ou x86_64 sur un Mac Intel
curl -sSfLO "https://github.com/kubewatch-io/kubewatch/releases/download/v${VERSION}/kubewatch-${VERSION}-${ARCH}-apple-darwin.tar.gz"
tar xzf "kubewatch-${VERSION}-${ARCH}-apple-darwin.tar.gz"
cd "kubewatch-${VERSION}-${ARCH}-apple-darwin"

cp -R KubeWatch.app /Applications/
```

L'archive macOS ne contient **que** le bundle : le binaire vit dans
`KubeWatch.app/Contents/MacOS/`, il n'y a rien à copier dans `/usr/local/bin`.

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

La fenêtre s'ouvre tout de suite, même si aucun cluster n'est joignable : la
reconnexion aux clusters enregistrés se fait en tâche de fond. Au premier
démarrage, il n'y en a aucun — l'écran Clusters propose d'importer un
kubeconfig (`$KUBECONFIG`, puis `~/.kube/config`) ou de déclarer une connexion
distante par URL et jeton.

### macOS : quarantaine Gatekeeper

Le bundle n'est ni signé ni notarisé — le projet ne dispose d'aucun certificat
Apple Developer. Au premier lancement, macOS refusera de l'ouvrir. Après avoir
vérifié la somme SHA-256 et la signature cosign :

```sh
xattr -dr com.apple.quarantine /Applications/KubeWatch.app
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

Compiler KubeWatch demande **deux chaînes d'outils** : celle de Rust pour le
binaire, celle de Node pour l'interface.

| Outil | Version | Pourquoi |
| --- | --- | --- |
| Rust | 1.85 ou plus récent | Édition 2021, MSRV du workspace |
| Node.js | 22 ou plus récent | Construire l'interface web de `ui/` |
| npm | Celui de Node | `package-lock.json` fige les versions |
| `tauri-cli` 2 | `cargo install tauri-cli --version '^2' --locked` | Nécessaire pour `make run` et `make bundles`, pas pour `make release` |

Sous Linux, il faut en plus les bibliothèques de la webview. **Elles sont liées
à la compilation** : sans elles, `cargo build` échoue à l'édition de liens.

```sh
# Debian / Ubuntu
sudo apt-get install libwebkit2gtk-4.1-dev libjavascriptcoregtk-4.1-dev \
  libsoup-3.0-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev

# Fedora
sudo dnf install webkit2gtk4.1-devel libsoup3-devel gtk3-devel \
  libappindicator-gtk3-devel librsvg2-devel

# Arch
sudo pacman -S webkit2gtk-4.1 libsoup3 gtk3 libayatana-appindicator librsvg

# Void
sudo xbps-install libwebkit2gtk41-devel libsoup3-devel gtk+3-devel
```

C'est la liste qu'installe la CI, et c'est elle qui fait foi. Deux d'entre eux —
`ayatana-appindicator` et `librsvg` — sont réclamés par `pkg-config` pendant la
compilation sans que le binaire produit ne les lie : `readelf -d` ne les
mentionne pas. Ils restent nécessaires pour compiler.

`rfd`, la bibliothèque du sélecteur de fichiers, est compilée en mode
`xdg-portal` et dialogue par D-Bus en Rust pur : aucun paquet supplémentaire
n'est requis pour lui.

### Compiler et lancer

```sh
git clone https://github.com/kubewatch-io/kubewatch
cd kubewatch

make setup      # dépendances npm de ui/
make run        # la fenêtre, avec le rechargement à chaud de Vite
make release    # le binaire optimisé (profil dist), interface incluse
make install    # dans ~/.cargo/bin
make dist       # l'archive du binaire nu, dans dist/
make bundles    # AppImage, .deb et .rpm, dans dist/ (Linux)
make appimage   # seulement l'AppImage
```

`make release` construit d'abord `ui/dist`, puis le binaire :
`tauri::generate_context!` exige que l'interface existe à la compilation.
`make run` passe par `cargo tauri dev`, qui démarre Vite et ouvre la fenêtre
dessus ; c'est le seul mode où l'interface n'est pas embarquée.

`make bundles` et `make appimage` délèguent à `tauri build`. Contrairement au
binaire nu, l'AppImage produit ainsi **embarque la webview** et ses dépendances,
ce qui le rend portable d'une distribution à l'autre. Détails dans
[packaging/README.md](../packaging/README.md#appimage-et-paquets-de-distribution).

La publication sur crates.io n'est pas encore activée (variable de dépôt
`PUBLISH_CRATES`) : `cargo install kubewatch-desktop` ne fonctionnera
qu'ensuite. En attendant, `cargo install --path crates/desktop` exige que
`ui/dist` ait été construit d'avance — `make install` s'en occupe.

### Compilation croisée

```sh
make release TARGET=aarch64-unknown-linux-gnu
```

Préférez une compilation **native** sur la machine cible, ou un conteneur de la
même architecture : les bibliothèques de la webview doivent être présentes pour
l'architecture visée, ce qu'une image `cross` ordinaire ne fournit pas. La
chaîne de release compile chaque cible sur un runner de son architecture.

---

## Mise à jour

L'application ne vérifie pas sa propre version et ne se met jamais à jour
toute seule. Suivez la page des releases du dépôt ; le remplacement consiste à
réinstaller l'archive de la nouvelle version, exactement comme la première fois
— c'est aussi l'occasion de revérifier la signature.

---

## Désinstallation

```sh
# Binaire
sudo rm -f /usr/local/bin/kubewatch-desktop
# ou, s'il a été installé par cargo :
cargo uninstall kubewatch-desktop

# Entrée de menu, métadonnées et icône
sudo rm -f /usr/share/applications/io.kubewatch.KubeWatch.desktop \
           /usr/share/metainfo/io.kubewatch.KubeWatch.metainfo.xml \
           /usr/share/icons/hicolor/*/apps/io.kubewatch.KubeWatch.*
# ou, pour une installation par utilisateur :
make desktop-uninstall

# État local : clusters enregistrés, surveillants, historique, jetons, clés d'IA
rm -rf ~/.local/share/kubewatch                    # Linux
rm -rf ~/Library/Application\ Support/kubewatch    # macOS
# Windows : %APPDATA%\kubewatch

# Données de la webview : préférences d'affichage et caches, aucun secret
rm -rf ~/.local/share/io.kubewatch.KubeWatch       # Linux
```

Sur macOS, supprimez aussi `/Applications/KubeWatch.app`.

Le dossier d'état contient des secrets (jetons GitHub, jetons de cluster, clés
d'API des fournisseurs d'IA) : supprimez-le si vous désinstallez pour de bon.

---

## Diagnostic

Lancez l'application depuis un terminal pour voir ce qu'elle raconte :

```sh
RUST_LOG=debug kubewatch-desktop
```

### Symptômes fréquents

| Symptôme | Piste |
| --- | --- |
| `error while loading shared libraries: libwebkit2gtk-4.1.so.0` | La webview n'est pas installée, ou seule la série 4.0 l'est : voir [Bibliothèques d'exécution](#bibliothèques-dexécution-sous-linux) |
| Erreur analogue sur `libsoup-3.0.so.0`, `libgtk-3.so.0`, `libjavascriptcoregtk-4.1.so.0` | Même cause, même remède : `readelf -d kubewatch-desktop` liste tout ce qui manque |
| Aucune fenêtre, `GDK_BACKEND`/`DISPLAY` dans le message | Aucune session graphique : vous êtes en SSH ou en console |
| « Parcourir… » ne fait rien | `xdg-desktop-portal` absent ou non démarré ; saisissez le chemin du kubeconfig à la main |
| Jauges CPU/mémoire vides, prévision de capacité indisponible | metrics-server absent : `kubectl top nodes` échoue aussi |
| `403` sur une action | Droits du kubeconfig insuffisants |
| `429` / `rateLimited` sur les registres ou les mises à jour | Renseignez un jeton GitHub dans la section « Mises à jour » de l'écran Réglages |
| La fenêtre ne se rattache pas à l'icône du lanceur | `StartupWMClass` du `.desktop` ne correspond pas à l'`app_id` réel — voir [packaging/README.md](../packaging/README.md#construire-un-paquet-de-distribution) |
| L'assistant répond « fournisseur injoignable » | Adresse de base erronée, ou serveur local (LM Studio, Ollama…) non démarré |

### Compiler échoue

| Message | Piste |
| --- | --- |
| `The system library 'webkit2gtk-4.1' required by crate 'webkit2gtk-sys' was not found` | Les paquets `-dev` manquent : voir [Dépendances de compilation](#dépendances-de-compilation) |
| `frontendDist` introuvable, ou `ui/dist` vide | Construisez l'interface d'abord : `make release` le fait, `cargo build` seul ne le fait pas |
| `tauri-cli absent` | `cargo install tauri-cli --version '^2' --locked` |

Voir aussi [configuration.md](configuration.md), [interface.md](interface.md) et
[security.md](security.md).
