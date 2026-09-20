# Sécurité

KubeWatch est une **application locale**. Elle tourne sous votre identité
d'utilisateur, lit vos fichiers, parle aux serveurs que vous lui désignez, et
n'ouvre aucun port. Son modèle de sécurité est celui de `kubectl`, pas celui
d'un service.

Ce document dit ce qui est protégé, ce qui ne l'est pas, et où sont vos secrets.

- [Le point essentiel](#le-point-essentiel)
- [Modèle de menace](#modèle-de-menace)
- [Portée des privilèges](#portée-des-privilèges)
- [Aucune surface d'écoute](#aucune-surface-découte)
- [La webview et son contenu](#la-webview-et-son-contenu)
- [Stockage local des secrets](#stockage-local-des-secrets)
- [Les secrets à l'écran](#les-secrets-à-lécran)
- [Ignorer la vérification TLS](#ignorer-la-vérification-tls)
- [Ce qui sort de votre machine](#ce-qui-sort-de-votre-machine)
- [Journalisation](#journalisation)
- [Chaîne d'approvisionnement](#chaîne-dapprovisionnement)
- [Limites connues](#limites-connues)
- [Liste de contrôle](#liste-de-contrôle)
- [Signaler une vulnérabilité](#signaler-une-vulnérabilité)

---

## Le point essentiel

**KubeWatch hérite exactement des droits du kubeconfig que vous lui donnez.**
Il n'ajoute aucune restriction, ne filtre aucune action, ne demande aucune
confirmation que le cluster n'exigerait pas. Si votre kubeconfig est
`cluster-admin`, l'application l'est aussi — y compris son bouton
« Supprimer ».

Le corollaire est rassurant : KubeWatch ne peut rien faire que vous ne
puissiez déjà faire avec `kubectl`, et tout ce qu'il fait apparaît dans le
journal d'audit de l'API server sous **votre** identité.

Depuis le pivot vers l'application de bureau, trois inquiétudes classiques ont
simplement disparu : il n'y a plus de port ouvert, plus de jeton d'API à
protéger, et plus de service à durcir.

---

## Modèle de menace

### Ce contre quoi KubeWatch se protège

| Menace | Réponse |
| --- | --- |
| Lecture des secrets locaux par un autre compte de la machine | Fichiers d'état en `0600`, dans un dossier `0700`, sous Unix |
| Fichier d'état corrompu par une coupure | Écriture atomique : fichier temporaire, puis renommage |
| Fuite d'un secret dans un affichage ou une notification | Les jetons stockés sont remplacés par `••••••` dès qu'ils sont réaffichés |
| Interception du trafic vers le cluster ou les registres | TLS obligatoire par défaut, via rustls, avec vérification du certificat |
| Chargement de code ou de contenu distant dans l'interface | CSP restrictive déclarée dans `crates/desktop/tauri.conf.json` : `default-src` et `script-src` limités à `'self'`, `connect-src` au seul canal IPC |
| Archive de release altérée | Sommes SHA-256 publiées et signées par cosign (identité OIDC du workflow) |
| Dépendance vulnérable | `cargo audit` (RustSec), `cargo deny`, `cargo vet`, en CI quotidienne |

### Ce contre quoi il ne protège pas

| Menace | Pourquoi |
| --- | --- |
| Un kubeconfig trop permissif | C'est votre RBAC qui décide, et lui seul. KubeWatch ne peut pas deviner ce que vous auriez voulu interdire |
| Un compte utilisateur compromis sur votre poste | Qui lit votre session lit votre kubeconfig, avec ou sans KubeWatch |
| Un chiffrement des secrets au repos | Les fichiers d'état ne sont pas chiffrés : ils sont protégés par les permissions du système de fichiers, comme `~/.kube/config` |
| Une erreur de manipulation | Les actions destructrices demandent confirmation dans l'interface, mais une confirmation donnée est exécutée |
| Un cluster hostile | Un API server malveillant peut renvoyer n'importe quoi ; KubeWatch affiche ce qu'il reçoit |

---

## Portée des privilèges

L'application utilise **le kubeconfig, tel quel**. Ni élévation, ni réduction.

```sh
# Ce que vous avez le droit de faire, du point de vue du cluster
kubectl auth can-i --list
kubectl auth can-i delete deployments -n production
```

Ce que renvoie `kubectl auth can-i` est exactement ce que pourra faire
KubeWatch. Si une action échoue en `403` dans l'application, c'est le RBAC qui
parle, et la solution est côté cluster.

### Travailler avec moins de droits

La bonne pratique, pour de l'observation quotidienne, est d'utiliser un contexte
restreint plutôt que votre contexte d'administration. Créez un ServiceAccount en
lecture seule, et fabriquez-en un kubeconfig :

```yaml
apiVersion: v1
kind: ServiceAccount
metadata:
  name: kubewatch-lecture
  namespace: default
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: kubewatch-lecture
rules:
  # Tout lire, ne rien écrire. Les « pods/log » sont séparés à dessein :
  # ils donnent accès au contenu des journaux applicatifs.
  - apiGroups: ["*"]
    resources: ["*"]
    verbs: ["get", "list", "watch"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: kubewatch-lecture
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: kubewatch-lecture
subjects:
  - kind: ServiceAccount
    name: kubewatch-lecture
    namespace: default
```

Avec un tel contexte, l'interface reste entièrement navigable : les actions
d'écriture échouent proprement en `403`, avec le message de l'API server.

Deux verbes méritent une décision explicite, parce qu'ils donnent accès au
**contenu** et non seulement à la structure :

| Verbe | Ce qu'il ouvre |
| --- | --- |
| `get` sur `pods/log` | Tout ce que vos applications écrivent dans leurs journaux |
| `create` sur `pods/exec` | Un shell dans le conteneur, avec les droits du processus |
| `get` sur `secrets` | La valeur des secrets, en clair après décodage base64 |

Si vous n'avez pas besoin du terminal, ne vous donnez pas `pods/exec`.

---

## Aucune surface d'écoute

C'est le changement le plus net depuis la version précédente.

- **Aucun port n'est ouvert**, ni sur la boucle locale, ni ailleurs. Il n'y a
  plus de serveur HTTP, plus d'API `/api`, plus de WebSocket, plus de route de
  webhook.
- **Aucun jeton d'API** n'est nécessaire, donc aucun ne peut fuiter, être
  deviné, ou rester en clair dans une variable d'environnement.
- **Aucune question d'exposition réseau** : il n'y a rien à exposer. Pas de
  proxy inverse à configurer, pas de TLS à terminer, pas de liste d'adresses
  autorisées à tenir.

Vérifiable en une commande, l'application étant lancée :

```sh
ss -tlnp | grep kubewatch     # ne doit rien afficher
```

Les connexions que vous verrez sont toutes **sortantes** : vers votre API
server, et vers les services listés plus bas.

L'ancienne route de webhook GitHub a disparu avec le serveur. Le module de
vérification HMAC-SHA256 subsiste dans la bibliothèque `kubewatch-updater`, mais
aucun binaire ne l'appelle — voir
[updates.md](updates.md#le-webhook-github-a-été-retiré).

---

## La webview et son contenu

L'interface de KubeWatch est une page web, affichée par la webview du système —
WebKitGTK sous Linux. C'est la contrepartie de l'éditeur de code, du terminal et
du Markdown de l'assistant, et elle mérite d'être regardée en face.

**Rien ne vient du réseau.** L'interface est compilée d'avance et **embarquée
dans le binaire** : elle est servie depuis la mémoire du processus, pas depuis
un serveur ni depuis un fichier modifiable. Aucun CDN, aucune police distante,
aucun script tiers chargé au démarrage.

**Une CSP l'impose, elle ne se contente pas de l'espérer.** Elle est déclarée
dans `crates/desktop/tauri.conf.json` et posée sur chaque page :

```
default-src 'self';
script-src  'self';
style-src   'self' 'unsafe-inline';
img-src     'self' data: blob:;
font-src    'self' data:;
connect-src ipc: http://ipc.localhost
```

Ce que cela veut dire, point par point :

- **aucun script distant** ne peut être chargé, et aucun script injecté depuis
  une adresse extérieure ne s'exécuterait ;
- **aucune requête réseau depuis la page** : `connect-src` n'autorise que le
  canal IPC de Tauri. Une réponse de l'assistant qui contiendrait un `fetch()`
  ne joindrait rien. Tout le trafic sortant part du **Rust**, où il est visible,
  typé et limité aux destinations listées plus bas ;
- **aucune image distante** : le catalogue d'applications embarqué porte des URL
  d'icônes vers `cdn.simpleicons.org`, mais l'interface ne les affiche pas — les
  cartes du catalogue utilisent une pastille dessinée localement — et `img-src`
  les refuserait de toute façon. Ces URL sont mentionnées ici parce qu'elles
  figurent dans le code, pas parce qu'elles génèrent du trafic ;
- `style-src` accepte `'unsafe-inline'`, ce qu'exigent les styles calculés par
  React. C'est le seul assouplissement, et il ne permet pas d'exécuter du code.

**Les permissions de Tauri sont réduites au strict nécessaire.**
`crates/desktop/capabilities/default.json` n'accorde à la fenêtre `main` que
`core:default` : évènements, canaux et fenêtre. Aucun plugin de système de
fichiers, de shell ou de processus n'est activé ; l'interface ne peut donc rien
faire d'autre qu'appeler les commandes que le Rust expose. Deux d'entre elles
touchent au système, et toutes deux sont bornées : `pick_file` ouvre le
sélecteur de fichiers de votre bureau, et `open_url` **refuse toute adresse qui
ne commence pas par `http://` ou `https://`** avant de la confier à votre
navigateur.

**Ce que cela ne protège pas.** La webview reste un moteur de rendu HTML
complet, avec sa propre surface d'attaque, et c'est celui du système : sa
sécurité dépend de la mise à jour de WebKitGTK par votre distribution. C'est un
argument de plus pour tenir le poste à jour.

---

## Stockage local des secrets

| Fichier | Contenu sensible |
| --- | --- |
| `<state_dir>/clusters.json` | Jetons de service, certificats et clés clientes des clusters distants |
| `<state_dir>/updater.json` | Jeton GitHub, secret de webhook résiduel |
| `<state_dir>/ai.json` | Clés d'API des fournisseurs d'IA (Anthropic, OpenAI, serveur compatible) |
| `~/.kube/config` | Le vôtre, lu mais jamais modifié |

Emplacements exacts : voir
[configuration.md](configuration.md#emplacements-par-défaut).

Garanties d'écriture :

- **écriture atomique** : fichier temporaire dans le même dossier, puis
  renommage ; une interruption ne laisse jamais un fichier tronqué ;
- **permissions `0600`** sous Unix — lecture et écriture par le propriétaire
  seul — appliquées aux trois fichiers d'état, dans un dossier créé en `0700` ;
- aucun secret n'est écrit dans les traces, quel que soit le niveau de
  `RUST_LOG`.

```sh
ls -ld ~/.local/share/kubewatch/
# drwx------ 2 vous vous  … /home/vous/.local/share/kubewatch/
ls -l ~/.local/share/kubewatch/
# -rw------- 1 vous vous  … ai.json
# -rw------- 1 vous vous  … clusters.json
# -rw------- 1 vous vous  … updater.json
```

**Sous Windows et macOS, il n'y a pas de trousseau.** Les secrets ne sont pas
déposés dans le Keychain ni dans le gestionnaire d'identifiants Windows : ils
sont dans les mêmes fichiers JSON. Sous Windows, les permissions POSIX ne
s'appliquent pas et les fichiers héritent des ACL de votre profil. Traitez ces
fichiers comme votre kubeconfig — parce que c'est exactement ce qu'ils valent.

**Un cluster ajouté est toujours écrit.** L'écran Clusters n'offre pas d'option
« garder en mémoire pour cette session » : import de kubeconfig comme connexion
distante, la spécification part dans `clusters.json`, jeton compris. Pour un
jeton de courte durée, la seule façon de ne pas le laisser derrière soi est de
retirer le cluster quand vous avez fini — ou de travailler dans un dossier
d'état jetable avec `KUBEWATCH_STATE_DIR`.

---

## Les secrets à l'écran

Il faut distinguer deux choses, parce qu'elles ne se comportent pas pareil.

**Les secrets de KubeWatch lui-même sont masqués.** Le jeton GitHub et le secret
de webhook sont remplacés par `••••••` partout où ils sont réaffichés, dans la
section « Mises à jour » de l'écran Réglages ; il en va de même de la clé d'API
d'un profil d'assistant. Renvoyer le masque tel quel conserve la valeur
existante au lieu de l'écraser : vous pouvez modifier un autre réglage sans
retaper votre jeton, et sans risquer de l'effacer.

**Les Secrets de Kubernetes, eux, ne le sont pas — délibérément.** Le
comportement dépend de ce que vous demandez :

| Où | Ce qui est affiché |
| --- | --- |
| Liste des ressources, ligne d'un `Secret` | Le type et le **nombre de clés**. Jamais une valeur |
| Panneau de détail, onglet YAML | L'objet complet, `data` compris, en base64 — exactement ce que renvoie `kubectl get secret -o yaml` |

Ce n'est pas un oubli : un outil d'administration qui masquerait le contenu d'un
Secret serait inutilisable pour diagnostiquer un Secret. Mais cela veut dire
qu'**afficher le YAML d'un Secret pendant un partage d'écran expose le
secret**, et qu'une capture d'écran le grave dans un fichier. Le base64 n'est
pas un chiffrement.

Si vous ne voulez pas courir ce risque, la réponse est côté cluster : ne vous
donnez pas `get` sur `secrets`.

---

## Ignorer la vérification TLS

La case « Ignorer la vérification TLS du serveur (déconseillé) » du formulaire
« Connexion à un serveur d'API », dans l'écran Clusters, désactive
**réellement** la vérification du certificat du serveur pour ce cluster. Ce
n'est pas un avertissement de forme.

Concrètement, une fois cochée :

- n'importe quel certificat est accepté, y compris auto-signé, expiré, ou émis
  pour un autre nom ;
- une machine placée entre vous et l'API server peut présenter son propre
  certificat, déchiffrer, lire et modifier tout le trafic — **y compris votre
  jeton d'authentification**, qu'elle pourra ensuite rejouer ;
- la protection s'évapore silencieusement : rien ne clignote une fois la
  connexion établie.

La bonne réponse, dans presque tous les cas, est de fournir l'autorité de
certification plutôt que de désactiver la vérification : collez son certificat
PEM dans le champ « Autorité de certification » du même formulaire, et laissez
la case décochée.

Le certificat de l'autorité d'un cluster se récupère depuis le kubeconfig
existant (`certificate-authority-data`, en base64) ou auprès de qui l'exploite.
`--insecure` reste acceptable sur un cluster jetable, sur un réseau que vous
maîtrisez, avec un jeton sans valeur. Jamais en production, jamais sur un réseau
partagé.

---

## Ce qui sort de votre machine

Rien n'est envoyé nulle part sans une action de votre part. Il n'y a **aucune
télémétrie**, aucun rapport d'erreur automatique, aucun appel au démarrage.

| Destination | Quand | Ce qui est envoyé | Comment s'en passer |
| --- | --- | --- | --- |
| Votre API server Kubernetes | En permanence pendant l'usage | Vos identifiants du kubeconfig, les requêtes que vous déclenchez | C'est le produit ; sans cela il n'y a rien à afficher |
| `api.github.com` | Vérification d'une surveillance GitHub, suggestions | Le nom du dépôt interrogé, votre jeton GitHub s'il est configuré | N'enregistrez pas de surveillance de type `githubRelease` |
| `hub.docker.com`, `registry-1.docker.io`, `ghcr.io`, `quay.io`, ou le registre que vous visez | Recherche, liste de tags, inspection d'image, surveillance de type registre | Le nom de l'image, les identifiants de registre si vous en avez fourni | N'utilisez pas l'écran Hub ; n'enregistrez pas de surveillance de type registre |
| `artifacthub.io` | Recherche de charts Helm | Le terme recherché | N'utilisez pas l'onglet Charts Helm |
| `api.anthropic.com`, `api.openai.com`, ou le serveur compatible OpenAI que vous désignez (LM Studio, Ollama…) | Chaque question posée à l'assistant IA | Votre question et l'historique de la conversation ; le contexte de l'écran : nom du cluster, namespace, objet sélectionné **et son YAML** ; et, si les outils sont activés, tout ce que le modèle demande à lire : listes d'objets, manifestes, évènements, **journaux de pods**, métriques. Un Secret Kubernetes lu par l'assistant part en clair (base64) chez le fournisseur. | Ne configurez aucun fournisseur ; ou désactivez les outils dans les réglages ; ou pointez un **modèle local** pour que rien ne quitte votre machine |
| `cdn.simpleicons.org` | Jamais (voir [La webview et son contenu](#la-webview-et-son-contenu)) | Rien | Sans objet |
| Le site que vous ouvrez depuis un lien | Quand vous cliquez un lien de l'interface ou d'une réponse de l'assistant | Rien de plus que ce qu'envoie votre navigateur : la page ne s'ouvre pas dans KubeWatch, mais dans le navigateur du système | Ne cliquez pas |

Trois précisions qui comptent :

- **Aucune donnée de votre cluster ne part vers ces services.** Ce sont des
  requêtes de lecture publiques : « quelles sont les versions de nginx ? ».
  Le nom de vos images, en revanche, est nécessairement transmis au registre
  que vous interrogez — c'est inhérent à la question posée.
- **Le jeton GitHub n'est envoyé qu'à `api.github.com`**, et sert uniquement à
  relever le quota de 60 à 5000 requêtes par heure. Un jeton
  « fine-grained » **sans aucune permission** suffit, puisque seules des données
  publiques sont lues. Ne donnez pas plus.
- **Les variables `HTTP_PROXY`, `HTTPS_PROXY` et `NO_PROXY` sont respectées**
  pour ces appels sortants : sur un réseau qui filtre, tout passe par votre
  mandataire.

Pour une machine totalement hors ligne, hormis le cluster : n'ouvrez ni le Hub
ni les Mises à jour. Rien d'autre ne sort.

---

## Journalisation

Les traces vont sur la **sortie d'erreur**, jamais dans un fichier écrit
automatiquement. Le niveau se règle par `RUST_LOG` :

```sh
RUST_LOG=debug kubewatch-desktop
RUST_LOG=info,kubewatch_core=debug kubewatch-desktop
```

Aucun jeton, aucun mot de passe, aucun contenu de Secret n'est écrit dans les
traces, à quelque niveau que ce soit. En revanche, `RUST_LOG=trace` fait
apparaître les URL appelées et les noms d'objets : si vous collez une trace dans
un ticket, relisez-la.

**Le journal qui fait foi est celui de l'API server.** C'est lui qui enregistre
qui a fait quoi, avec votre identité. KubeWatch ne tient pas de journal d'audit,
et ne prétend pas le faire.

---

## Chaîne d'approvisionnement

Ce qui est en place, vérifiable dans
[`.github/workflows/security.yml`](../.github/workflows/security.yml) :

- **`cargo audit`** (base RustSec) sur chaque push et chaque jour ;
- **`cargo deny`** : licences autorisées, sources autorisées, interdictions,
  doublons (`deny.toml`) — aucune dépendance sous copyleft fort ;
- **`cargo vet`**, informatif tant qu'il n'est pas initialisé ;
- **`package-lock.json`** pour l'interface : les versions npm sont figées, et la
  CI installe avec `npm ci`, qui refuse de s'écarter du verrou ;
- **CodeQL** sur les workflows GitHub Actions ;
- **sommes SHA-256 signées par cosign** à chaque release, en mode *keyless* :
  l'identité du workflow est attestée par OIDC auprès de Fulcio, et aucune clé
  privée n'existe — donc aucune ne peut fuiter.

Ce qui **n'est pas** en place, et qu'il faut savoir :

- **Aucun audit automatique des dépendances npm.** `cargo audit` couvre la
  moitié Rust ; rien d'équivalent ne tourne sur l'arbre de `ui/`. C'est le prix
  d'une interface web, et il est énoncé plutôt que passé sous silence.
  L'interface étant compilée d'avance et confinée par la CSP, une dépendance
  compromise ne pourrait pas joindre le réseau depuis la page — mais elle
  aurait accès à tout ce que l'interface affiche, et pourrait appeler les
  commandes que le backend expose.
- **Pas de notarisation Apple, pas de signature Authenticode.** Les binaires
  déclencheront Gatekeeper et SmartScreen. La vérification cosign est votre
  seule garantie d'origine — faites-la.
- **Plus de SBOM ni d'attestation de provenance** : elles étaient produites pour
  l'image conteneur, qui n'existe plus.
- **Aucune mise à jour automatique de KubeWatch** : l'application ne vérifie
  ni ne remplace sa propre version. Le remplacement du binaire se fait à la
  main, avec la procédure de vérification de
  [installation.md](installation.md#vérifier-lintégrité-et-la-signature).

---

## Limites connues

Énoncées franchement, pour que vous puissiez décider en connaissance de cause :

1. **Les secrets locaux ne sont pas chiffrés au repos.** Ils sont protégés par
   les permissions du système de fichiers, pas par un trousseau.
2. **Le YAML d'un Secret Kubernetes est affiché tel quel**, base64 compris.
   Attention aux partages d'écran.
3. **La case « ignorer la vérification TLS » désactive réellement la
   vérification** du cluster concerné, avec toutes les conséquences décrites
   plus haut.
4. **Aucune mise à jour automatique de KubeWatch** : rien ne vérifie ni ne
   remplace le binaire ; c'est à vous de le faire, signature comprise.
5. **Aucune validation d'admission côté KubeWatch** : un manifeste appliqué
   depuis la console YAML est envoyé tel quel à l'API server, seul arbitre.
6. **Aucun journal d'audit local.** Ce qui a été fait depuis l'application n'est
   traçable que dans le journal d'audit du cluster.
7. **L'assistant IA envoie des données du cluster au fournisseur choisi**, à
   chaque question : ce que vous sélectionnez et ce que ses outils lisent
   (jusqu'aux journaux et aux Secrets). Pour un cluster sensible, utilisez un
   modèle local, ou désactivez les outils. Ses réponses sont des suggestions :
   rien n'est appliqué sans un clic de votre part.
8. **Aucune identité applicative.** L'application ne sait pas qui est devant
   l'écran : elle sait seulement sous quel compte système elle tourne.
9. **Les binaires ne sont pas signés au sens du système d'exploitation.**
10. **Une seule instance à la fois** doit écrire les fichiers d'état ; deux
    processus qui écrivent en parallèle peuvent perdre la modification la plus
    ancienne.
11. **L'interface s'affiche dans la webview du système**, dont la sécurité
    dépend des mises à jour de votre distribution. La CSP interdit tout contenu
    et tout appel réseau depuis la page, mais le moteur de rendu reste une
    surface d'attaque qui n'appartient pas au projet.
12. **Les dépendances npm de l'interface ne sont pas auditées automatiquement**,
    contrairement aux dépendances Rust.
13. **Un cluster ajouté est systématiquement écrit sur le disque** : il n'existe
    pas de connexion valable pour la seule session en cours.

---

## Liste de contrôle

Avant d'utiliser KubeWatch sur un cluster qui compte :

- [ ] Le kubeconfig employé est celui dont vous avez besoin, pas le plus
      puissant dont vous disposez.
- [ ] `kubectl auth can-i --list` a été relu pour ce contexte.
- [ ] `pods/exec` et `get secrets` ne sont accordés que si vous en avez l'usage.
- [ ] Les fichiers d'état sont bien en `0600` et leur dossier en `0700`
      (`ls -l`, `ls -ld`).
- [ ] La case « Ignorer la vérification TLS du serveur » n'est cochée sur aucun
      cluster de production.
- [ ] Les clusters dont vous n'avez plus l'usage ont été retirés : leur jeton
      reste sinon dans `clusters.json`.
- [ ] Le jeton GitHub est un jeton *fine-grained* sans aucune permission.
- [ ] Le journal d'audit de l'API server est actif — c'est lui qui fait foi.
- [ ] L'archive téléchargée a été vérifiée : `sha256sum -c` **et**
      `cosign verify-blob`.
- [ ] Le poste lui-même est à jour et verrouillé : c'est désormais la principale
      frontière de sécurité.

---

## Signaler une vulnérabilité

Ne créez pas d'issue publique. Utilisez la fonction **« Report a vulnerability »**
de l'onglet *Security* du dépôt
[kubewatch-io/kubewatch](https://github.com/kubewatch-io/kubewatch/security),
qui ouvre un avis privé.

Merci d'inclure : la version concernée, les étapes de reproduction, l'impact
constaté, et le contexte (système, session graphique, version de la webview du
système, mode de connexion au cluster).
