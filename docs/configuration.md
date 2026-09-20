# Configuration

KubeWatch n'a ni option de ligne de commande, ni fichier de configuration :
`kubewatch-desktop` s'ouvre, et tout se règle depuis ses écrans Réglages et
Clusters. Quelques variables d'environnement suffisent aux cas particuliers.

- [Ce que l'application lit au lancement](#ce-que-lapplication-lit-au-lancement)
- [Variables d'environnement](#variables-denvironnement)
- [Emplacements par défaut](#emplacements-par-défaut)
- [Ce qui est écrit sur le disque](#ce-qui-est-écrit-sur-le-disque)

---

## Ce que l'application lit au lancement

| Source | Rôle |
| --- | --- |
| Le dossier d'état | Les clusters enregistrés (`clusters.json`), l'état du détecteur de mises à jour (`updater.json`) : surveillants, détections, historique, jeton GitHub, et les profils de l'assistant IA (`ai.json`) |
| Le stockage local de la webview | Les préférences de l'interface : thème, cluster courant, namespace retenu par cluster, écran ouvert, type de ressource affiché, actualisation automatique, ouverture et largeur du panneau de l'assistant |
| Le kubeconfig | Seulement quand vous l'importez depuis l'écran Clusters : `$KUBECONFIG`, sinon `~/.kube/config`, ou le fichier que vous désignez |

Rien n'est lu ailleurs. En particulier, le fichier `config.yaml` de l'ancienne
ligne de commande n'est plus consulté : ce qu'il réglait (contexte, namespace,
jeton GitHub) se choisit maintenant dans l'interface.

Aucune lecture de cluster n'a lieu pendant la construction de l'application :
la reconnexion aux clusters enregistrés part en tâche de fond, pour que la
fenêtre s'affiche sans attendre un serveur injoignable. Si un fichier d'état est
illisible, l'application démarre sur un état vide et l'annonce en notification.

---

## Variables d'environnement

| Variable | Rôle |
| --- | --- |
| `KUBEWATCH_STATE_DIR` | Dossier d'état à utiliser à la place de l'emplacement par défaut. Pratique pour isoler un profil de test, ou dans un environnement sans `HOME` |
| `RUST_LOG` | Verbosité des traces, écrites sur la sortie d'erreur. Sans elle, le filtre est `info,kube=warn,hyper=warn,tower=warn,h2=warn` ; `debug`, ou un filtre comme `info,kubewatch_core=debug`, le remplacent entièrement |
| `KUBECONFIG` | Kubeconfig proposé par défaut à l'import ; plusieurs chemins séparés par `:` (`;` sous Windows) sont acceptés |
| `HTTPS_PROXY`, `HTTP_PROXY`, `NO_PROXY` | Mandataire pour les appels sortants vers les registres d'images, GitHub et les fournisseurs d'IA. Pour le serveur d'API Kubernetes, c'est le champ `proxy-url` du kubeconfig, ou le champ « Proxy HTTP / SOCKS5 » du formulaire de connexion distante, qui compte |

En fonctionnement dans un cluster, `kubewatch-core` lit aussi
`KUBERNETES_SERVICE_HOST` et `KUBERNETES_SERVICE_PORT` pour détecter une
configuration in-cluster ; ce n'est pas le cas d'usage d'une application de
bureau, mais le code de connexion est le même.

```sh
RUST_LOG=debug KUBEWATCH_STATE_DIR=/tmp/kubewatch-test kubewatch-desktop
```

Les variables `KUBEWATCH_*` de l'ancienne ligne de commande
(`KUBEWATCH_NAMESPACE`, `KUBEWATCH_CONTEXT`, `KUBEWATCH_GITHUB_TOKEN`, …) ne
sont plus lues. Le jeton GitHub se saisit dans la section « Mises à jour » de
l'écran Réglages.

`KUBEWATCH_HELM_BIN` n'est plus lue non plus par l'application : la fonction de
rendu de charts qui l'honorait vit toujours dans la bibliothèque
`kubewatch-hub`, mais **aucune commande de l'application ne l'appelle**. Le
binaire `helm` n'est donc plus nécessaire, ni cherché.

Enfin, `TAURI_DEV_HOST` et les variables `TAURI_ENV_*` ne concernent que la
compilation : `tauri-cli` les pose pour Vite, elles n'ont aucun effet sur un
binaire déjà construit.

---

## Emplacements par défaut

Le dossier d'état est `<dossier de données de l'utilisateur>/kubewatch`, avec
deux replis : `~/.kubewatch`, puis `./.kubewatch` dans le répertoire courant si
aucun dossier de données n'est déterminable. Fixez plutôt `KUBEWATCH_STATE_DIR`
dans ce dernier cas.

| Système | Dossier d'état |
| --- | --- |
| Linux | `$XDG_DATA_HOME/kubewatch`, sinon `~/.local/share/kubewatch` |
| macOS | `~/Library/Application Support/kubewatch` |
| Windows | `%APPDATA%\kubewatch` |

Ce dossier est celui des versions précédentes : une installation existante est
reprise telle quelle.

Le **stockage de la webview** est séparé, et porte l'identifiant de
l'application plutôt que son nom court. Sous Linux, c'est
`~/.local/share/io.kubewatch.KubeWatch/` : `localstorage/` (les préférences de
l'interface), `WebKitCache/`, `CacheStorage/`, `storage/` et
`hsts-storage.sqlite`. Sous macOS et Windows, ce stockage appartient à la
webview du système ; il n'est ni choisi ni configuré par KubeWatch.

Effacer ce dossier ne fait perdre que les préférences d'affichage : aucun
cluster, aucun surveillant et aucune clé n'y sont écrits.

---

## Ce qui est écrit sur le disque

| Fichier | Contenu | Protection |
| --- | --- | --- |
| `<dossier d'état>/clusters.json` | Clusters enregistrés, y compris jetons, certificats et clés clientes des connexions distantes | `0600` sous Unix, dossier `0700`, écriture atomique |
| `<dossier d'état>/updater.json` | Surveillants, détections, historique, jeton GitHub, secret de webhook résiduel | `0600` sous Unix, écriture atomique |
| `<dossier d'état>/ai.json` | Profils de l'assistant IA : fournisseur, adresse, modèle, **clé d'API en clair**, réglages des outils | `0600` sous Unix, écriture atomique |
| Stockage local de la webview | Préférences de l'interface, sous la clé `kubewatch-ui` | Aucun secret |

Les trois fichiers JSON contiennent des secrets : traitez-les comme un
kubeconfig. Le détail est dans [security.md](security.md).

Un cluster importé ou déclaré depuis l'écran Clusters est **toujours écrit dans
`clusters.json`** : il n'y a pas d'option « garder en mémoire pour cette
session ». Pour un jeton de courte durée, retirez le cluster quand vous avez
fini.

La géométrie de la fenêtre n'est pas conservée : `tauri.conf.json` fixe la
taille d'ouverture et la fenêtre s'affiche centrée à chaque lancement.
