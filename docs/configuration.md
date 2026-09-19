# Configuration

KubeWatch n'a ni option de ligne de commande, ni fichier de configuration :
`kubewatch-desktop` s'ouvre, et tout se règle depuis son écran Réglages. Deux
variables d'environnement suffisent aux cas particuliers.

- [Ce que l'application lit au lancement](#ce-que-lapplication-lit-au-lancement)
- [Variables d'environnement](#variables-denvironnement)
- [Emplacements par défaut](#emplacements-par-défaut)
- [Ce qui est écrit sur le disque](#ce-qui-est-écrit-sur-le-disque)

---

## Ce que l'application lit au lancement

| Source | Rôle |
| --- | --- |
| Le dossier d'état | Les clusters enregistrés (`clusters.json`) et l'état du détecteur de mises à jour (`updater.json`) : surveillances, détections, historique, jeton GitHub |
| Le stockage d'eframe | Les préférences de l'interface : thème, densité, zoom, largeur du panneau de navigation, rafraîchissement automatique, dernier cluster, dernier écran, taille de page, lignes de journal conservées, confirmation des opérations destructrices, géométrie de la fenêtre |
| Le kubeconfig | Seulement quand vous l'importez depuis l'écran Réglages : `$KUBECONFIG`, sinon `~/.kube/config`, ou le fichier que vous désignez |

Rien n'est lu ailleurs. En particulier, le fichier `config.yaml` de l'ancienne
ligne de commande n'est plus consulté : ce qu'il réglait (contexte, namespace,
jeton GitHub) se choisit maintenant dans l'interface, et le namespace courant
n'est pas conservé d'une session à l'autre.

---

## Variables d'environnement

| Variable | Rôle |
| --- | --- |
| `KUBEWATCH_STATE_DIR` | Dossier d'état à utiliser à la place de l'emplacement par défaut. Pratique pour isoler un profil de test, ou dans un environnement sans `HOME` |
| `RUST_LOG` | Verbosité des traces, écrites sur la sortie d'erreur : `info` par défaut, `debug`, ou un filtre comme `info,kubewatch_core=debug` |
| `KUBECONFIG` | Kubeconfig proposé par défaut à l'import ; plusieurs chemins séparés par `:` (`;` sous Windows) sont acceptés |
| `KUBEWATCH_HELM_BIN` | Chemin d'un binaire `helm` précis pour le rendu des charts, à la place de celui trouvé dans le `PATH` |
| `HTTPS_PROXY`, `HTTP_PROXY`, `NO_PROXY` | Mandataire pour les appels sortants vers les registres d'images et GitHub. Pour le serveur d'API Kubernetes, c'est le champ `proxy-url` du kubeconfig qui compte |
| `WGPU_BACKEND`, `WGPU_POWER_PREF` | Lues par la couche graphique elle-même, pour le diagnostic ; voir [installation.md](installation.md#lapplication-de-bureau) |

Les variables `KUBEWATCH_*` de l'ancienne ligne de commande (`KUBEWATCH_NAMESPACE`,
`KUBEWATCH_CONTEXT`, `KUBEWATCH_GITHUB_TOKEN`, …) ne sont plus lues. Le jeton
GitHub se saisit dans l'onglet Réglages de l'écran Mises à jour.

```sh
RUST_LOG=debug KUBEWATCH_STATE_DIR=/tmp/kubewatch-test kubewatch-desktop
```

---

## Emplacements par défaut

| Système | Dossier d'état | Préférences de l'interface |
| --- | --- | --- |
| Linux | `$XDG_DATA_HOME/kubewatch`, sinon `~/.local/share/kubewatch` | `~/.local/share/KubeWatch/app.ron` |
| macOS | `~/Library/Application Support/kubewatch` | `~/Library/Application Support/KubeWatch/app.ron` |
| Windows | `%APPDATA%\kubewatch` | `%APPDATA%\KubeWatch\data\app.ron` |

Le dossier d'état est celui des versions précédentes : une installation
existante est reprise telle quelle. Si aucun dossier de données n'est
déterminable, KubeWatch se replie sur `~/.kubewatch`, puis sur `./.kubewatch`
dans le répertoire courant ; fixez plutôt `KUBEWATCH_STATE_DIR` dans ce cas.

L'emplacement des préférences est choisi par eframe, d'après le nom de
l'application ; il ne se configure pas.

---

## Ce qui est écrit sur le disque

| Fichier | Contenu | Protection |
| --- | --- | --- |
| `<dossier d'état>/clusters.json` | Clusters enregistrés, y compris jetons et certificats des connexions distantes | `0600` sous Unix, dossier `0700`, écriture atomique |
| `<dossier d'état>/updater.json` | Surveillances, détections, historique, jeton GitHub | `0600` sous Unix, écriture atomique |
| `app.ron` (voir ci-dessus) | Préférences de l'interface et géométrie de la fenêtre | Aucun secret |

Les deux fichiers JSON contiennent des secrets : traitez-les comme un
kubeconfig. Un cluster enregistré avec la case « Enregistrer la connexion sur
le disque » décochée n'y apparaît jamais. Le détail est dans
[security.md](security.md).
