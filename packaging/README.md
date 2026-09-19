# Empaquetage de KubeWatch

Ce dossier contient les métadonnées de bureau et la marche à suivre pour
empaqueter KubeWatch sur chaque système.

| Fichier | Rôle |
| --- | --- |
| [`io.kubewatch.KubeWatch.desktop`](io.kubewatch.KubeWatch.desktop) | Entrée de menu freedesktop (Linux, BSD) |
| [`io.kubewatch.KubeWatch.metainfo.xml`](io.kubewatch.KubeWatch.metainfo.xml) | Métadonnées AppStream pour les logithèques |

Le projet produit **un seul binaire** :

| Binaire | Crate | Nature |
| --- | --- | --- |
| `kubewatch-desktop` | `crates/desktop` | Application de bureau (egui/eframe, backend wgpu) |

**Ce qui n'existe pas encore, et qu'il ne faut donc pas annoncer :** aucun
paquet `.deb`, `.rpm`, Flatpak, Snap, MSI ni formule Homebrew n'est publié.
Aucune icône n'est livrée. Aucun binaire n'est signé, ni sur macOS ni sur
Windows. Les recettes ci-dessous sont manuelles et vérifiables ; elles ne
décrivent pas une chaîne automatisée qui n'existe pas.

- [Linux](#linux)
- [macOS](#macos)
- [Windows](#windows)
- [Icône](#icône)
- [Valider les métadonnées](#valider-les-métadonnées)

---

## Linux

### Ce que les archives de release contiennent

Les archives `…-unknown-linux-gnu.tar.gz` contiennent `kubewatch-desktop`, la
licence, la documentation, `io.kubewatch.KubeWatch.desktop` et
`io.kubewatch.KubeWatch.metainfo.xml`.

Il n'y a pas d'archive musl. Une application graphique ouvre Vulkan, OpenGL,
Wayland, X11 et xkbcommon par `dlopen` à l'exécution : un binaire musl statique
n'a pas le chargeur dynamique qu'il faut pour cela. Livrer un `kubewatch-desktop`
musl reviendrait à livrer un artefact qui ne s'ouvre pas.

### Installation manuelle

```sh
# Binaires
sudo install -m 0755 kubewatch kubewatch-desktop /usr/local/bin/

# Entrée de menu et métadonnées de logithèque
sudo install -Dm 0644 io.kubewatch.KubeWatch.desktop \
  /usr/share/applications/io.kubewatch.KubeWatch.desktop
sudo install -Dm 0644 io.kubewatch.KubeWatch.metainfo.xml \
  /usr/share/metainfo/io.kubewatch.KubeWatch.metainfo.xml

# Rafraîchir les caches du bureau (facultatif : la plupart des environnements
# les régénèrent seuls, mais l'entrée peut sinon mettre du temps à apparaître)
sudo update-desktop-database /usr/share/applications || true
```

Pour une installation par utilisateur, sans `sudo` : `~/.local/bin`,
`~/.local/share/applications` et `~/.local/share/metainfo`.

### Dépendances d'exécution

Aucune de ces bibliothèques n'est liée à la compilation : toutes sont ouvertes
par `dlopen` au démarrage. En conséquence, **leur absence ne se voit qu'au
lancement**, pas à l'installation — un paquet bien construit les déclare donc
explicitement.

| Bibliothèque ouverte | Debian / Ubuntu | Fedora | Arch | Quand |
| --- | --- | --- | --- | --- |
| `libwayland-client.so.0` | `libwayland-client0` | `wayland-libs-client` | `wayland` | Session Wayland |
| `libxkbcommon.so.0` | `libxkbcommon0` | `libxkbcommon` | `libxkbcommon` | Clavier, toujours |
| `libxkbcommon-x11.so.0` | `libxkbcommon-x11-0` | `libxkbcommon-x11` | `libxkbcommon-x11` | Session X11 |
| `libX11.so.6` | `libx11-6` | `libX11` | `libx11` | Session X11 |
| `libXcursor.so.1` | `libxcursor1` | `libXcursor` | `libxcursor` | Session X11 |
| `libXrandr.so.2` | `libxrandr2` | `libXrandr` | `libxrandr` | Session X11 |
| `libXi.so.6` | `libxi6` | `libXi` | `libxi` | Session X11 |
| `libvulkan.so.1` | `libvulkan1` | `vulkan-loader` | `vulkan-icd-loader` | Rendu wgpu / Vulkan |
| `libEGL.so.1`, `libGL.so.1` | `libegl1`, `libgl1` | `libglvnd-egl`, `libglvnd-glx` | `libglvnd` | Repli wgpu / OpenGL |

Il faut en outre un **pilote** : `mesa-vulkan-drivers` (Debian/Ubuntu),
`mesa-vulkan-drivers` (Fedora), `vulkan-radeon` / `vulkan-intel` (Arch), ou le
pilote propriétaire de votre carte. Sans pilote, wgpu ne trouve aucun adaptateur
et la fenêtre ne s'ouvre pas.

Enfin, les sélecteurs de fichiers passent par le **portail XDG**
(`xdg-desktop-portal` plus l'implémentation de votre bureau :
`xdg-desktop-portal-gtk`, `-kde`, `-hyprland`…). Sans portail en service, le
reste de l'application fonctionne, mais les boîtes de dialogue « ouvrir un
fichier » n'apparaissent pas.

### Dépendances de compilation

Rien de particulier : ni GTK, ni Qt, ni webkit2gtk. KubeWatch n'embarque pas de
webview. Avec la résolution de features du dépôt, aucune bibliothèque graphique
n'est réclamée par `pkg-config`, ce qui se vérifie ainsi après une compilation :

```sh
# Vide => le build.rs de wayland-sys est sorti sans appeler pkg-config,
# parce que la feature « dlopen » est active.
cat target/debug/build/wayland-sys-*/output

# Ne doit mentionner que « dl » : x11-dl charge X11 à l'exécution.
cat target/debug/build/x11-dl-*/output
```

Les paquets `-dev` installés par la CI (`libxkbcommon-dev`,
`libxkbcommon-x11-dev`, `libwayland-dev`, `libx11-dev`, `libxcursor-dev`,
`libxrandr-dev`, `libxi-dev`, `libgl1-mesa-dev`, `libegl1-mesa-dev`) le sont à
titre de filet de sécurité : si la feature `dlopen` disparaissait de la
résolution, `wayland-sys` repasserait par `pkg-config` et exigerait
`libwayland-dev` sans autre forme de procès.

### Construire un paquet de distribution

Aucun paquet n'est maintenu en amont. Les points à connaître si vous en montez
un :

- `Exec=kubewatch-desktop` suppose le binaire dans le `PATH`. Si votre paquet
  l'installe ailleurs (`/opt/kubewatch/bin`, par exemple), remplacez la ligne
  par un chemin absolu.
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
- Pour **Flatpak**, le portail XDG est déjà présent dans l'environnement, mais
  l'accès au kubeconfig ne l'est pas : il faut au minimum
  `--filesystem=~/.kube:ro` (et `~/.local/share/kubewatch` en écriture pour
  l'état), ainsi que `--share=network` et `--device=dri`. Aucun manifeste
  Flatpak n'est fourni ni testé.

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
    └── Resources/          (vide : aucune icône n'est fournie)
```

Reproduire l'opération localement :

```sh
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

**Aucune icône n'est livrée.** `io.kubewatch.KubeWatch.desktop` déclare
`Icon=io.kubewatch.KubeWatch` ; tant qu'aucun fichier ne porte ce nom dans un
thème d'icônes, le bureau affiche un pictogramme générique. L'entrée de menu
fonctionne quand même.

Pour en ajouter une, sans modifier ces fichiers :

```sh
# Vectorielle (préférable)
sudo install -Dm 0644 kubewatch.svg \
  /usr/share/icons/hicolor/scalable/apps/io.kubewatch.KubeWatch.svg
# Ou matricielle, une taille par dossier
sudo install -Dm 0644 kubewatch-256.png \
  /usr/share/icons/hicolor/256x256/apps/io.kubewatch.KubeWatch.png
sudo gtk-update-icon-cache /usr/share/icons/hicolor || true
```

L'icône de la **fenêtre** (barre des tâches, Alt-Tab) est un sujet distinct :
elle est posée par l'application au démarrage, pas par ce fichier.

---

## Valider les métadonnées

Les deux fichiers sont validés à chaque exécution de la CI (job
« Métadonnées de bureau »). En local :

```sh
sudo apt-get install desktop-file-utils appstream   # Debian/Ubuntu

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
