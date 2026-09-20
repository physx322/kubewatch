# Empaquetage de KubeWatch

Ce dossier contient les métadonnées de bureau, l'icône et la marche à suivre
pour empaqueter KubeWatch sur chaque système.

| Fichier | Rôle |
| --- | --- |
| [`io.kubewatch.KubeWatch.desktop`](io.kubewatch.KubeWatch.desktop) | Entrée de menu freedesktop (Linux, BSD) |
| [`io.kubewatch.KubeWatch.metainfo.xml`](io.kubewatch.KubeWatch.metainfo.xml) | Métadonnées AppStream pour les logithèques |
| `icons/hicolor/` | L'icône, dans la disposition d'un thème freedesktop |

Le projet produit **un seul binaire** :

| Binaire | Crate | Nature |
| --- | --- | --- |
| `kubewatch-desktop` | `crates/desktop` | Application de bureau Tauri 2 : cœur Rust, interface web de `ui/` embarquée à la compilation, webview du système pour l'affichage |

**Ce qui n'existe pas encore, et qu'il ne faut donc pas annoncer :** aucun
paquet `.deb`, `.rpm`, AppImage, Flatpak, Snap, MSI ni formule Homebrew n'est
**publié** par la chaîne de release, qui ne livre que les archives du binaire
nu. `make bundles` en produit localement, et c'est tout. Aucun binaire n'est
signé, ni sur macOS ni sur Windows. Les recettes ci-dessous sont manuelles et
vérifiables ; elles ne décrivent pas une chaîne automatisée qui n'existe pas.

- [Linux](#linux)
- [macOS](#macos)
- [Windows](#windows)
- [Icône](#icône)
- [Valider les métadonnées](#valider-les-métadonnées)

---

## Linux

### Ce que les archives de release contiennent

Les archives `…-unknown-linux-gnu.tar.gz` contiennent `kubewatch-desktop`,
`LICENSE`, `README.md`, `CHANGELOG.md`, `io.kubewatch.KubeWatch.desktop`,
`io.kubewatch.KubeWatch.metainfo.xml`, le dossier `icons/hicolor/` et une copie
de ce fichier sous le nom `INSTALL-packaging.md`.

Le binaire embarque l'interface web : il n'y a **aucun fichier HTML, CSS ou
JavaScript à installer à côté**. Ce qu'il n'embarque pas, en revanche, c'est la
webview — voir [Dépendances d'exécution](#dépendances-dexécution).

Il n'y a pas d'archive musl. Le binaire est lié à WebKitGTK, GTK 3, libsoup 3 et
GLib, qui sont des bibliothèques partagées du système : un exécutable statique
ne peut pas les emporter, et n'aurait rien à afficher.

### Installation manuelle

```sh
# Binaire
sudo install -m 0755 kubewatch-desktop /usr/local/bin/

# Entrée de menu et métadonnées de logithèque
sudo install -Dm 0644 io.kubewatch.KubeWatch.desktop \
  /usr/share/applications/io.kubewatch.KubeWatch.desktop
sudo install -Dm 0644 io.kubewatch.KubeWatch.metainfo.xml \
  /usr/share/metainfo/io.kubewatch.KubeWatch.metainfo.xml

# Icône (voir la section « Icône » plus bas)
sudo cp -r icons/hicolor /usr/share/icons/

# Rafraîchir les caches du bureau (facultatif : la plupart des environnements
# les régénèrent seuls, mais l'entrée peut sinon mettre du temps à apparaître)
sudo update-desktop-database /usr/share/applications || true
sudo gtk-update-icon-cache /usr/share/icons/hicolor || true
```

Pour une installation par utilisateur, sans `sudo` : `~/.local/bin`,
`~/.local/share/applications`, `~/.local/share/metainfo` et
`~/.local/share/icons`. Depuis un clone du dépôt, `make desktop-install` fait
tout cela.

### Dépendances d'exécution

Toutes ces bibliothèques sont **liées à l'édition de liens** : elles figurent
dans les entrées `NEEDED` de l'exécutable. Leur absence n'attend pas l'ouverture
de la fenêtre pour se signaler — le chargeur dynamique refuse de démarrer le
programme, en nommant le fichier manquant. Un paquet de distribution doit donc
toutes les déclarer.

La liste se relit à tout moment sur le binaire :

```sh
readelf -d kubewatch-desktop | grep NEEDED
```

| Bibliothèque liée | Debian / Ubuntu | Fedora | Arch | Rôle |
| --- | --- | --- | --- | --- |
| `libwebkit2gtk-4.1.so.0` | `libwebkit2gtk-4.1-0` | `webkit2gtk4.1` | `webkit2gtk-4.1` | La webview qui affiche l'interface |
| `libjavascriptcoregtk-4.1.so.0` | `libjavascriptcoregtk-4.1-0` | `webkit2gtk4.1-jsc` | `webkit2gtk-4.1` | Le moteur JavaScript de la webview |
| `libsoup-3.0.so.0` | `libsoup-3.0-0` | `libsoup3` | `libsoup3` | Pile HTTP de la webview |
| `libgtk-3.so.0`, `libgdk-3.so.0` | `libgtk-3-0` | `gtk3` | `gtk3` | La fenêtre elle-même |
| `libgdk_pixbuf-2.0.so.0` | `libgdk-pixbuf-2.0-0` | `gdk-pixbuf2` | `gdk-pixbuf2` | Chargement des images de GTK |
| `libcairo.so.2` | `libcairo2` | `cairo` | `cairo` | Rendu 2D de GTK |
| `libglib-2.0.so.0`, `libgobject-2.0.so.0`, `libgio-2.0.so.0` | `libglib2.0-0` | `glib2` | `glib2` | Socle de GTK et de la webview |
| `libdbus-1.so.3` | `libdbus-1-3` | `dbus-libs` | `dbus` | Bus de session, dont le portail XDG |
| `libgcc_s.so.1`, `libm.so.6`, `libc.so.6` | `libc6`, `libgcc-s1` | `glibc`, `libgcc` | `glibc`, `gcc-libs` | Bibliothèque C, toujours présente |

**Attention aux renommages `t64`.** Sur Ubuntu 24.04 et Debian 13, la
transition vers `time_t` 64 bits a renommé plusieurs de ces paquets :
`libgtk-3-0t64`, `libglib2.0-0t64`, `libgdk-pixbuf-2.0-0`… Vérifiez le nom
exact sur la distribution que vous ciblez plutôt que de recopier la colonne.

**Deux dépendances de compilation ne se retrouvent pas ici.**
`ayatana-appindicator` et `librsvg` sont réclamés par `pkg-config` pendant la
compilation, mais le binaire produit ne les lie pas : ils n'apparaissent pas
dans les entrées `NEEDED`. Un paquet n'a donc pas à les exiger à l'exécution.

Enfin, le sélecteur de fichiers passe par le **portail XDG**
(`xdg-desktop-portal` plus l'implémentation de votre bureau :
`xdg-desktop-portal-gtk`, `-kde`, `-hyprland`…). `rfd` est compilé en mode
`xdg-portal` et dialogue par D-Bus en Rust pur. Sans portail en service, le
reste de l'application fonctionne, mais le bouton « Parcourir… » de l'import de
kubeconfig n'ouvre rien ; le chemin reste saisissable à la main.

### Dépendances de compilation

Il en faut deux jeux : la chaîne Node pour l'interface, et les bibliothèques de
développement de la webview pour le binaire.

```sh
# Node.js 22+ et npm, pour construire ui/
node --version && npm --version

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

C'est la liste qu'installe la CI, et c'est elle qui fait foi. Contrairement aux
versions précédentes, ce n'est pas un filet de sécurité : sans ces paquets,
`cargo build` échoue au moment où `webkit2gtk-sys` interroge `pkg-config`.

```sh
# Ce que le build.rs de webkit2gtk-sys a trouvé, après une compilation :
cat target/debug/build/webkit2gtk-sys-*/output
```

L'autre exigence est propre à Tauri : `tauri::generate_context!` lit le dossier
déclaré par `frontendDist` — `ui/dist` — **à la compilation**. Il doit donc
exister avant `cargo build`. `make release` construit l'interface d'abord ; pour
compiler ou tester le Rust sans reconstruire l'interface, un `index.html` vide
suffit, ce que fait la cible `ui-dist` du Makefile et ce que fait la CI.

### AppImage et paquets de distribution

`make bundles` (soit `cargo tauri build` dans `crates/desktop`) construit
l'interface, puis le binaire, puis les paquets déclarés par
`crates/desktop/tauri.conf.json` : sous Linux, **AppImage, `.deb` et `.rpm`**.
Ils atterrissent dans `target/release/bundle/`, et la cible les recopie dans
`dist/` avec leur `.sha256`. `make appimage` fait la même chose en se limitant à
l'AppImage.

```sh
make bundles                  # les trois formats
make appimage                 # seulement l'AppImage
make bundles BUNDLES=deb      # seulement le .deb
```

Points à connaître :

- **L'AppImage embarque la webview.** C'est `linuxdeploy` qui l'assemble, et
  Tauri le télécharge ainsi que le runtime AppImage à la première exécution.
  C'est ce qui rend l'image portable d'une distribution à l'autre — et ce qui
  explique sa taille, sans commune mesure avec le binaire nu.
- Ces paquets utilisent le profil **release**, pas le profil `dist` de
  `make release` : les chemins de sortie diffèrent
  (`target/release/bundle/` contre `target/dist/`).
- La glibc de l'hôte doit rester au moins aussi récente que celle de la machine
  qui a construit l'image :

  ```sh
  objdump -T target/release/kubewatch-desktop | grep -o 'GLIBC_[0-9.]*' | sort -Vu | tail -1
  ```

  Construisez donc sur la distribution la plus ancienne que vous visez, ou dans
  un conteneur qui l'imite.
- Le runtime AppImage monte l'image au lancement avec `libfuse.so.2`. Sans
  FUSE : `./KubeWatch-….AppImage --appimage-extract-and-run`, ou
  `--appimage-extract` pour la déballer une bonne fois dans `squashfs-root/`.
- Aucune information de mise à jour (`zsync`) n'est embarquée, et l'AppImage ne
  s'intègre pas de lui-même au menu : c'est le rôle d'un outil comme
  AppImageLauncher ou `appimaged`.
- Pas de signature GPG dans l'image ; seul le `.sha256` l'accompagne.
- Ces paquets ne sont **pas publiés** par la chaîne de release.

### Construire un paquet de distribution

Aucun paquet n'est maintenu en amont. Les points à connaître si vous en montez
un, à la main plutôt qu'avec `tauri build` :

- `Exec=kubewatch-desktop` suppose le binaire dans le `PATH`. Si votre paquet
  l'installe ailleurs (`/opt/kubewatch/bin`, par exemple), remplacez la ligne
  par un chemin absolu.
- Déclarez toutes les bibliothèques de la table
  [Dépendances d'exécution](#dépendances-dexécution), et recommandez
  `xdg-desktop-portal` plus une implémentation.
- La convention AppStream veut que l'entrée de bureau porte le nom de
  l'identifiant : `io.kubewatch.KubeWatch.desktop`. Si vous renommez le
  fichier, mettez à jour `<launchable type="desktop-id">` dans le metainfo, sans
  quoi la logithèque ne fera plus le lien entre les deux.
- `StartupWMClass=io.kubewatch.KubeWatch` doit correspondre à l'identifiant
  d'application réellement déclaré par la fenêtre (`app_id` sous Wayland,
  `WM_CLASS` sous X11). Vérifiez-le une fois l'application lancée, sinon la
  fenêtre ne se rattachera pas à l'icône du lanceur :

  ```sh
  xprop WM_CLASS                      # X11 : cliquez sur la fenêtre
  # Wayland : l'app_id est visible dans les outils de votre compositeur,
  # par exemple « swaymsg -t get_tree » sous sway.
  ```
- Pour **Flatpak**, il faut un runtime qui fournit WebKitGTK 4.1 et GTK 3 ; le
  portail XDG est déjà présent dans l'environnement, mais l'accès au kubeconfig
  ne l'est pas : il faut au minimum `--filesystem=~/.kube:ro` (et
  `~/.local/share/kubewatch` en écriture pour l'état), ainsi que
  `--share=network` et `--device=dri`. Aucun manifeste Flatpak n'est fourni ni
  testé.

---

## macOS

### Bundle `.app`

Le workflow de release assemble un bundle minimal, écrit à la main :

```
KubeWatch.app/
└── Contents/
    ├── Info.plist          (CFBundleIdentifier io.kubewatch.KubeWatch)
    ├── PkgInfo
    ├── MacOS/
    │   └── kubewatch-desktop
    └── Resources/          (vide : ce bundle ne porte pas d'icône)
```

Reproduire l'opération localement — en construisant l'interface d'abord, sans
quoi la compilation échoue :

```sh
make ui-build
cargo build --release -p kubewatch-desktop

APP=dist/KubeWatch.app
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/kubewatch-desktop "$APP/Contents/MacOS/"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>KubeWatch</string>
  <key>CFBundleIdentifier</key><string>io.kubewatch.KubeWatch</string>
  <key>CFBundleExecutable</key><string>kubewatch-desktop</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
plutil -lint "$APP/Contents/Info.plist"
```

`cargo tauri build` produit un bundle plus complet, icône comprise, mais ce
n'est pas celui que publie la chaîne de release. La webview vient du système
(WKWebView) : aucun paquet à installer.

### Signature et notarisation

**Le bundle n'est ni signé ni notarisé.** Le projet ne dispose d'aucun
certificat Apple Developer. Conséquence concrète : au premier lancement,
Gatekeeper refuse d'ouvrir l'application. Pour l'utiliser malgré tout :

```sh
xattr -dr com.apple.quarantine /Applications/KubeWatch.app
```

C'est une manipulation que l'utilisateur doit faire en connaissance de cause :
elle désactive une vérification de sécurité réelle. Vérifiez la somme SHA-256 et
la signature cosign de l'archive **avant** de retirer la quarantaine.

Il n'existe pas de `.dmg` ni de formule Homebrew.

---

## Windows

L'archive `…-x86_64-pc-windows-msvc.zip` contient `kubewatch-desktop.exe`. Il
n'y a **ni installeur, ni MSI, ni signature Authenticode** : SmartScreen
affichera un avertissement au premier lancement.

La webview vient du système (WebView2, présent par défaut sur Windows 11 et
déployé par Windows Update sur Windows 10) : rien à installer à côté du binaire.

`kubewatch-desktop.exe` est compilé avec `windows_subsystem = "windows"` en
release : lancé depuis l'Explorateur, il n'ouvre pas de fenêtre de console
noire derrière lui. En contrepartie, il n'écrit rien sur la sortie standard —
pour voir ses traces, lancez-le depuis un terminal avec `RUST_LOG=debug`.

ARM64 Windows n'est pas publié : la cible ne pourrait être compilée que de
façon croisée depuis un runner x86-64, où rien ne permet de vérifier qu'une
fenêtre s'ouvre.

Installation simple :

```powershell
$dest = "$env:LOCALAPPDATA\Programs\KubeWatch"
New-Item -ItemType Directory -Force -Path $dest | Out-Null
Copy-Item .\kubewatch-desktop.exe $dest
# Ajouter $dest au PATH de l'utilisateur, puis créer un raccourci vers
# kubewatch-desktop.exe dans le menu Démarrer.
```

---

## Icône

Il y a **deux jeux d'icônes**, pour deux usages distincts.

`packaging/icons/hicolor/` est l'icône du **thème freedesktop**, celle que
lisent le menu et la logithèque :

| Fichier | Rôle |
| --- | --- |
| `scalable/apps/io.kubewatch.KubeWatch.svg` | La source vectorielle |
| `256x256/apps/io.kubewatch.KubeWatch.png` | Le rendu matriciel, produit par `make icons` (ImageMagick avec le délégué librsvg) |

`io.kubewatch.KubeWatch.desktop` la déclare par `Icon=io.kubewatch.KubeWatch` ;
il suffit donc de copier le dossier dans un thème :

```sh
sudo cp -r packaging/icons/hicolor /usr/share/icons/     # système
make desktop-install                                     # utilisateur courant
sudo gtk-update-icon-cache /usr/share/icons/hicolor || true
```

`crates/desktop/icons/` porte les icônes que **Tauri** utilise : celles que
`tauri.conf.json` énumère pour les paquets (`32x32.png`, `128x128.png`,
`128x128@2x.png`, `icon.icns`, `icon.ico`) et celle de la fenêtre. Elles se
régénèrent d'un fichier source unique :

```sh
cd crates/desktop && cargo tauri icon ../../packaging/icons/hicolor/scalable/apps/io.kubewatch.KubeWatch.svg
```

Si vous changez le dessin, changez les deux jeux : rien ne les synchronise
automatiquement.

---

## Valider les métadonnées

Les deux fichiers sont validés à chaque exécution de la CI (job
« Métadonnées de bureau »). En local :

```sh
sudo apt-get install desktop-file-utils appstream   # Debian/Ubuntu

make packaging-check
# ou, à la main :
desktop-file-validate packaging/io.kubewatch.KubeWatch.desktop
appstreamcli validate --no-net packaging/io.kubewatch.KubeWatch.metainfo.xml
```

Résultat attendu aujourd'hui : `desktop-file-validate` ne signale qu'un *hint*
sur la catégorie `Monitor`, et `appstreamcli` termine par
`Validation was successful: pedantic: 1` — l'unique remarque « pedantic »
portant sur l'absence de captures d'écran, que le projet ne publie pas encore.

`--no-net` est important : sans cette option, `appstreamcli` tente de joindre
chaque `<url>` du fichier et transforme une panne passagère de `github.com` en
échec de validation.
