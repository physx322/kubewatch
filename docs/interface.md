# Guide de l'interface

Ce document décrit l'application de bureau `kubewatch-desktop`, écran par écran.
Le projet ne publie pas encore de captures : tout est décrit en texte, ce qui a
l'avantage de rester exact quand les couleurs changent.

- [La fenêtre](#la-fenêtre)
- [Raccourcis clavier](#raccourcis-clavier)
- [La barre supérieure](#la-barre-supérieure)
- [Le panneau de navigation](#le-panneau-de-navigation)
- [La barre d'état](#la-barre-détat)
- [Écran Vue d'ensemble](#écran-vue-densemble)
- [Écran Ressources](#écran-ressources)
- [Le panneau de détail](#le-panneau-de-détail)
- [Écran Topologie](#écran-topologie)
- [Écran Console YAML](#écran-console-yaml)
- [Écran Déployer](#écran-déployer)
- [Écran Catalogue](#écran-catalogue)
- [Écran Mises à jour](#écran-mises-à-jour)
- [Écran Réglages](#écran-réglages)
- [Ce que l'interface ne fait pas](#ce-que-linterface-ne-fait-pas)

---

## La fenêtre

La disposition ne change jamais : vous savez toujours où regarder.

```
┌──────────────────────────────────────────────────────────────────────────┐
│ KubeWatch │ ▼ cluster │ ▼ namespace │ filtre…    thème  auto  Rafraîchir │  barre supérieure
├───────────────┬──────────────────────────────────────────────────────────┤
│   Vue d'ens.  │                                                          │
│   Ressources  │                                                          │
│   Topologie   │                                                          │
│   Console YAML│                  zone centrale                           │
│   Déployer    │             (l'écran sélectionné)                        │
│   Catalogue   │                                                          │
│   Mises à jour│                                                          │
│   Réglages    │                                                          │
│ ───────────── │                                                          │
│ ● prod        │                                                          │
│   connecté    │                                                          │
├───────────────┴──────────────────────────────────────────────────────────┤
│ prod · v1.31.4 · production · 2 requête(s) : Liste des pods  …  F1 : aide │  barre d'état
└──────────────────────────────────────────────────────────────────────────┘
```

Le panneau de navigation se **redimensionne à la souris** (entre 150 et 420
pixels) et sa largeur est conservée d'une session à l'autre.

Deux principes valent partout :

- **Rien ne gèle.** Toute opération réseau part dans un fil séparé. Pendant
  qu'un cluster réfléchit — ou refuse de répondre pendant trente secondes — la
  fenêtre reste vivante : vous pouvez changer d'écran, annuler, ou en lancer une
  autre.
- **L'application reste utilisable sans cluster joignable.** Aucun écran ne se
  vide, aucune boîte de dialogue ne bloque : un message décrit ce qui s'est
  passé et la vie continue.

---

## Raccourcis clavier

| Raccourci | Effet |
| --- | --- |
| `Ctrl+1` … `Ctrl+8` | Changer d'écran, dans l'ordre de la barre de navigation |
| `Ctrl+R` | Rafraîchir l'écran courant |
| `Ctrl+K` | Ouvrir la recherche de type de ressource |
| `Échap` | Fermer, dans l'ordre : la recherche, l'aide, la confirmation, le panneau de détail |
| `Ctrl+Q` | Quitter |
| `F1` | Afficher ou masquer l'aide des raccourcis |

Sur macOS, `Ctrl` se lit **`Cmd`** : le modificateur employé est celui de la
plateforme.

`Ctrl+K` ouvre une **palette de types**. Tapez `deploy`, `svc`, `po`, un nom
complet ou une abréviation : la liste se réduit à mesure, `Entrée` retient le
premier résultat, et l'écran Ressources s'ouvre dessus. C'est le chemin le plus
court pour atteindre n'importe quel type, y compris les CRD de vos opérateurs.

Dans le terminal du panneau de détail, les frappes sont transmises au
conteneur — `Ctrl+C` interrompt le processus distant, `↑` et `↓` rappellent les
commandes précédentes. Les raccourcis globaux du tableau ci-dessus restent
prioritaires.

---

## La barre supérieure

De gauche à droite :

| Élément | Rôle |
| --- | --- |
| **Sélecteur de cluster** | Les clusters enregistrés. `●` connecté, `○` injoignable. Changer de cluster recharge la découverte des types, les namespaces et l'écran courant |
| **Sélecteur de namespace** | « Tous les namespaces », ou l'un d'eux. Le changement relance la liste immédiatement |
| **Champ de filtre** | Filtre **local** du tableau : nom, namespace, statut. Il ne déclenche aucune requête réseau — c'est instantané |
| **Soleil / lune** | Bascule thème clair / thème sombre ; l'icône montre le thème vers lequel on bascule |
| **⟳ auto** | Rafraîchissement automatique de l'écran courant. La période est réglable (10 s par défaut) |
| **Rafraîchir** | Recharge l'écran courant, comme `Ctrl+R` |
| **Indicateur d'activité** | Une roue tourne tant qu'une requête est en vol |

---

## Le panneau de navigation

Les huit écrans, dans l'ordre des raccourcis `Ctrl+1` à `Ctrl+8`. Survoler une
entrée affiche son raccourci. Chaque entrée porte une icône de la police
[Phosphor](https://phosphoricons.com/) : jauge, tableau, graphe, accolades,
fusée, boutique, flèche montante, engrenage.

En dessous, l'**état du cluster courant** :

- son nom, et une pastille verte (connecté) ou rouge (injoignable) ;
- la version du serveur d'API ;
- l'URL du serveur, tronquée, complète au survol ;
- la dernière erreur rencontrée, s'il y en a une — tronquée, complète au
  survol ;
- la disponibilité des mesures : « mesures disponibles » ou « mesures
  indisponibles », selon que `metrics-server` répond ou non.

Sans cluster enregistré, le panneau affiche « Aucun cluster » et renvoie vers
l'écran Réglages.

---

## La barre d'état

Une ligne, en bas, toujours visible : cluster courant · version du serveur ·
namespace · requêtes en vol · dernier message.

Les **requêtes en vol** sont nommées : « 2 requête(s) : Liste des pods, Synthèse
du cluster ». C'est le meilleur endroit pour comprendre ce que l'application
attend quand elle semble lente — et pour constater que la lenteur vient du
cluster, pas de l'interface.

À droite, le dernier message (tronqué, complet au survol) et le rappel
« F1 : aide ».

---

## Écran Vue d'ensemble

`Ctrl+1` — la synthèse du cluster courant : nœuds prêts, pods par état,
namespaces, charges de travail, mémoire, et les derniers **avertissements** du
cluster (type, objet, raison, message).

La synthèse est demandée dès que vous arrivez sur l'écran avec un cluster
sélectionné, et à chaque changement de cluster. Pendant la collecte, l'écran
affiche « ⟳ Collecte de la synthèse… » ; s'il n'y a rien à montrer — première
ouverture, ou tentative précédente en échec — un bouton « Charger la synthèse »
relance la demande.

**Sans aucun cluster enregistré**, cet écran devient un écran d'accueil qui
propose trois façons de commencer :

1. importer le kubeconfig du système (`$KUBECONFIG`, puis `~/.kube/config`) ;
2. choisir un fichier kubeconfig avec le sélecteur de fichiers ;
3. ouvrir les Réglages pour déclarer une connexion distante (URL + jeton).

Le sélecteur de fichiers passe par le portail XDG sous Linux ; s'il ne s'ouvre
pas, voir
[installation.md](installation.md#lapplication-de-bureau).

---

## Écran Ressources

`Ctrl+2` — le tableau de n'importe quel type d'objet.

**En-tête de l'écran** : le type affiché (`pods` par défaut), le sélecteur de
labels envoyé au serveur d'API, le filtre local, et le bouton « Rafraîchir ».
Le type se change avec `Ctrl+K`.

**Le tableau** : une ligne par objet, colonnes adaptées au type (namespace, nom,
prêt, statut, âge, nœud, image, adresse, ports, hôtes…). Les en-têtes sont
**cliquables** pour trier ; le tri comme le filtre s'appliquent localement, sur
les lignes déjà reçues.

Quand la liste est paginée, un bouton « Charger la suite » demande la page
suivante. Le nombre d'objets par page se règle dans les Réglages (500 par
défaut).

**Actions** : un **clic droit** sur une ligne ouvre le menu contextuel. Les
entrées qui n'ont pas de sens pour le type affiché sont grisées plutôt que
cachées — vous voyez ce qui existe, et pourquoi c'est indisponible.

| Action | Ce qu'elle fait | Disponible pour |
| --- | --- | --- |
| Voir le YAML | Ouvre le panneau de détail sur l'onglet YAML | Tout objet |
| Modifier | Ouvre le manifeste dans l'éditeur, pour le renvoyer au cluster | Tout objet |
| Journaux | Ouvre le panneau de détail sur le flux de journaux | Pods |
| Terminal | Ouvre une session interactive dans un conteneur | Pods |
| Redémarrer | `rollout restart` de la charge de travail | Deployment, StatefulSet, DaemonSet |
| Scaler… | Demande le nombre de répliques, puis l'applique | Objets scalables |
| Changer l'image… | Demande la nouvelle image, pré-remplie avec l'actuelle | Tout objet portant une image |
| Rollback | Revient à la révision précédente | Charges de travail versionnées |
| Supprimer | Supprime l'objet, avec choix de la politique de propagation | Tout objet |

Les actions destructrices ouvrent une **confirmation**, qui rappelle l'objet
visé. Cette confirmation peut être désactivée dans les Réglages ; elle est
active par défaut, et c'est un réglage à changer en connaissance de cause.

Quand la liste est vide, l'écran le dit plutôt que de laisser un tableau nu :
« Aucun objet « pods » dans ce cluster », avec un bouton pour élargir la
recherche à tous les namespaces ou effacer le filtre.

---

## Le panneau de détail

Il s'ouvre sur un objet sélectionné et se ferme avec `Échap`. Cinq onglets :

| Onglet | Contenu |
| --- | --- |
| **YAML** | Le manifeste complet, coloré syntaxiquement, `managedFields` retiré. Modifiable : les modifications sont renvoyées au cluster quand vous appliquez |
| **Évènements** | Les évènements concernant cet objet : horodatage, type, raison, message |
| **Conteneurs** | Les conteneurs du pod, conteneurs d'initialisation compris : image, état, prêt, redémarrages |
| **Journaux** | Le journal d'un conteneur, en flux continu |
| **Terminal** | Un shell interactif dans un conteneur |

Il affiche aussi les **labels**, les **annotations** et les **propriétaires** de
l'objet ; le propriétaire est cliquable, ce qui permet de remonter d'un pod à
son ReplicaSet puis à son Deployment.

### Journaux

« Démarrer » ouvre le flux, « Arrêter » le referme. Les lignes arrivent au fil de
l'eau. Le conteneur se choisit dans une liste, et les options habituelles sont
là : nombre de lignes initiales, horodatage, conteneur précédent.

Un **filtre** masque les lignes qui ne contiennent pas le texte saisi, sans
interrompre le flux. Le retour à la ligne automatique et le défilement collé au
bas sont deux commutateurs ; le défilement se décroche dès que vous remontez
dans l'historique, et se recolle quand vous redescendez.

Le tampon est **borné** (5 000 lignes par défaut, réglable) : un conteneur
bavard ne fait pas gonfler la mémoire indéfiniment. Les lignes les plus
anciennes sont perdues — pour un historique complet, `kubectl logs … > fichier`.

### Terminal

« Connecter » ouvre une session `exec` avec TTY dans le conteneur choisi. La
commande lancée est modifiable : `/bin/sh` par défaut, ce qui marche dans
presque toutes les images, y compris les distributions minimales. Le terminal
transmet les frappes telles quelles, y compris `Ctrl+C`, `Ctrl+U`, `Tab` et les
flèches, et suit le redimensionnement de la fenêtre.

C'est un terminal simple, pas un émulateur complet : il affiche le flux de
sortie du conteneur et gère l'historique de la ligne courante. Une application
plein écran (`vim`, `htop`, `top`) ne s'affichera pas correctement — passez par
`kubectl exec -it` dans votre terminal pour ces cas-là.

---

## Écran Topologie

`Ctrl+3` — le graphe des objets du cluster courant et de leurs liens, pour
répondre d'un coup d'œil à « ce pod appartient à quoi, qui l'expose, où
tourne-t-il ? ».

Chaque objet est une carte : sa famille et son namespace en petit, son nom, le
compteur `prêts/total` et son statut coloré comme partout ailleurs (vert en
marche, orange transitoire, rouge en erreur, bleu terminé). Une bande de
couleur sur le bord gauche reprend le statut, pour lire l'état du cluster même
en zoom arrière.

Les liens sont déduits de l'état réel, jamais devinés :

| Lien | De → vers | Source |
| --- | --- | --- |
| possède (trait plein) | Deployment → ReplicaSet → Pod ; StatefulSet, DaemonSet → Pod ; CronJob → Job → Pod | `metadata.ownerReferences` |
| sélectionne (tirets) | Service → Pod | `spec.selector` confronté aux labels du pod |
| route vers (tirets) | Ingress → Service | backends des règles et backend par défaut |
| planifié sur (pointillés) | Pod → Nœud | `spec.nodeName` |
| monte (pointillés) | Pod → PersistentVolumeClaim | volumes du pod |
| utilise (pointillés) | Pod → ConfigMap, Secret | volumes, `envFrom`, `valueFrom` |

**Se déplacer** : la molette (ou le pincement, ou `Ctrl` + molette) zoome
autour du pointeur, glisser le fond déplace la vue, « Ajuster » cadre tout le
graphe sans dépasser l'échelle 1:1. Le rendu se fait à la taille finale, donc
le texte reste net à tous les zooms. En zoom arrière, les cartes ne gardent que
leur nom, puis seulement leur couleur de statut. Glisser une carte la déplace et
l'**épingle** : les dispositions ne la toucheront plus, jusqu'à « Réorganiser »
ou « Détacher » dans son menu.

**Disposition** :

- *Couches* (par défaut) — une colonne par famille, de gauche à droite : Ingress,
  Services, charges de travail, ReplicaSets et Jobs, Pods, volumes et
  configuration, nœuds. L'ordre des lignes limite les croisements et chaque
  carte s'aligne sur ses voisines. Déterministe : deux lectures du même cluster
  donnent le même dessin.
- *Organique* — placement par forces : les objets liés se rapprochent, les
  autres se repoussent. Le dessin se stabilise en quelques secondes ; au-delà
  de 1 200 objets, cette disposition est désactivée.

**Couches** : chaque famille se masque d'un clic. Deux sont masquées par
défaut — les **ReplicaSets** (le Deployment est alors relié directement à ses
pods ; un ReplicaSet sans Deployment reste visible, c'est lui la charge de
travail) et la **configuration** (ConfigMaps et Secrets, qui encombrent vite).
« Masquer les isolés » ne garde que les objets reliés à au moins un autre.

**Lire** : survoler une carte met en avant son voisinage et atténue le reste ;
l'infobulle donne statut, compteur, redémarrages, nœud, âge et images. Le champ
de filtre de la barre supérieure fait la même chose pour tout ce qui
correspond (nom, namespace, statut, famille) et ses voisins directs. Un clic
ouvre le **panneau de détail** (YAML, évènements, conteneurs, journaux,
terminal) sans quitter le graphe. Le menu contextuel propose aussi « Voir dans
Ressources », « Centrer la vue ici », « Épingler » et « Copier le nom ».

**Ce qui manque se voit** : un objet cité par un autre mais absent de la
lecture — backend d'ingress qui ne pointe sur aucun service, ConfigMap qui
n'existe pas, nœud interdit par les droits — apparaît quand même, en gris et
marqué « non listé ». C'est souvent la raison pour laquelle on vient ici.

La lecture suit le **sélecteur de namespace** de la barre supérieure et se
rafraîchit comme les autres écrans (`Ctrl+R`, rafraîchissement automatique).
Les cartes ne bougent pas au rafraîchissement : seul un objet nouveau déclenche
un placement. Onze types sont lus en parallèle ; un type absent du cluster ou
interdit par les droits s'affiche comme avertissement au survol du compteur,
sans empêcher le reste.

---

## Écran Console YAML

`Ctrl+4` — un éditeur de manifestes multi-documents, coloré syntaxiquement.

**Charger le manifeste** : « Ouvrir… » (sélecteur de fichiers `.yaml`/`.yml`),
« Coller » (presse-papiers), ou en écrivant directement. « Enregistrer… » écrit
l'éditeur dans un fichier.

**Deux options, toujours visibles :**

- **Dry-run** — le manifeste part au serveur en simulation. Rien n'est modifié,
  mais le serveur valide vraiment, admission comprise.
- **Force** — reprend la propriété d'un champ détenu par un autre gestionnaire.
  À n'utiliser qu'en connaissance de cause : c'est ce qui permet d'écraser ce
  qu'un opérateur gère.

**Quatre actions :**

| Bouton | Effet |
| --- | --- |
| **Vérifier** | Analyse le YAML localement et annonce le nombre de documents valides. Aucun appel réseau |
| **Appliquer** | Server-Side Apply, gestionnaire de champs `kubewatch`, document par document |
| **Diff** | Compare le manifeste à l'état réel du cluster et affiche les différences, ressource par ressource |
| **Supprimer** | Supprime du cluster les ressources décrites par le manifeste, après confirmation |

Le compte rendu indique, **document par document**, ce qui a été créé,
configuré, laissé inchangé, simulé, supprimé ou refusé — avec le message du
serveur en cas d'échec.

Un **historique des applications** conserve les manifestes envoyés pendant la
session : « Rappeler » en recharge un dans l'éditeur. Il n'est pas écrit sur le
disque et disparaît à la fermeture.

Le champ *namespace* de la console s'applique aux documents qui n'en déclarent
pas, exactement comme `kubectl apply -n`.

---

## Écran Déployer

`Ctrl+5` — un assistant en quatre étapes, pour mettre une application sur le
cluster sans écrire de YAML ni connaître les champs d'un `Deployment`.

Le fil d'étapes en haut est cliquable : on revient en arrière à tout moment, on
ne saute pas une étape qu'on n'a pas encore atteinte.

### ① Quoi

Deux sources, au choix :

- **Catalogue** — la grille des applications embarquées, filtrable par texte et
  par catégorie. Chaque carte annonce son port, son besoin de volume et le
  nombre de réglages à renseigner. Un clic la retient et passe à l'étape
  suivante.
- **Mon image** — une référence d'image quelconque, vérifiée à la frappe.
  « Lire l'image » interroge le registre et récupère les **ports exposés**, les
  variables d'environnement par défaut et la plate-forme, qui pré-remplissent
  alors le déploiement.

### ② Comment

Cinq blocs, tous pré-remplis avec des valeurs raisonnables :

| Bloc | Ce qu'on y règle |
| --- | --- |
| **Identité** | Nom, namespace (les namespaces du cluster sont proposés), nombre de répliques |
| **Taille** | Un profil parmi *Micro*, *Petit*, *Moyen*, *Grand* — chacun affiche les quantités CPU et mémoire qu'il applique |
| **Accès** | *Interne au cluster*, *Port sur les nœuds*, *Adresse IP publique* ou *Nom de domaine* — traduits en type de Service et en Ingress. Puis les ports écoutés |
| **Stockage** | Une case, une taille et un point de montage : c'est tout ce qu'un `PersistentVolumeClaim` demande ici |
| **Réglages** | Les variables d'environnement de l'application |

« Réglages avancés… » reprend le déploiement dans le formulaire complet de
l'écran Catalogue, sans rien perdre, pour ce que l'assistant n'expose pas :
commande, arguments, placement sur les nœuds, secret de registre. Le chemin
inverse existe aussi, par le bouton « Assistant » de ce formulaire.

### ③ Vérification

C'est l'étape qui distingue cet écran d'un formulaire :

- **Ce qui sera créé** — la liste des objets Kubernetes, nommés et décrits en
  clair : Deployment, Service, Ingress, PersistentVolumeClaim.
- **Consommation prévue** — pour le processeur et la mémoire, une barre qui
  montre la part déjà occupée du cluster et, à sa suite, ce que ce déploiement
  réserve. Chiffres à l'appui : occupé, réservé, capacité totale, pourcentage
  après déploiement et place restante. Un verdict en tête dit si le
  déploiement **tient**, **passe mais charge le cluster** (au-delà de 85 %) ou
  **dépasse la capacité**.
- **À savoir** — les remarques que la prévision a levées, la plus grave en
  tête : limite inférieure à la demande (Kubernetes refuserait l'objet), volume
  `ReadWriteOnce` partagé entre plusieurs répliques, image sur un tag mouvant,
  absence de demande de ressources, réplique unique…
- **Manifeste Kubernetes** — le YAML multi-documents, repliable et copiable.
  Rien n'est appliqué sans qu'il ait pu être lu.

L'empreinte additionnée est celle des **demandes** (`resources.requests`), car
c'est sur elles que l'ordonnanceur réserve la place. Le point de départ est la
**consommation mesurée** du cluster, qui vient de `metrics-server`. Comparer les
deux donne un ordre de grandeur, pas une simulation d'ordonnanceur : les
réservations déjà consenties aux autres applications n'apparaissent pas dans une
mesure de consommation. Sans `metrics-server`, les barres disparaissent et
l'écran le dit — le déploiement, lui, reste possible.

### ④ Résultat

Le bilan de l'application, document par document. « Simuler d'abord » envoie les
manifestes en `dry-run` côté serveur : ils sont validés, rien n'est écrit, et le
bouton « Déployer pour de bon » attend juste à côté.

---

## Écran Catalogue

`Ctrl+6` — quatre onglets.

### Images

Recherche sur **Docker Hub**, **GHCR**, **Quay** ou un registre OCI générique.
Choisir une image affiche ses **tags** (filtrables), et sa **fiche détaillée** :
empreinte (digest), architecture, ports exposés, variables d'environnement,
entrypoint, labels — lus directement dans le manifeste et la configuration OCI.

`ghcr.io` et les registres génériques n'exposent pas d'API de recherche : pour
eux, saisissez la référence complète de l'image, qui sera vérifiée directement.

### Charts

Recherche de charts Helm sur **Artifact Hub**, avec leurs versions et leurs
`values.yaml`.

Le **rendu** d'un chart est délégué au binaire `helm` de votre machine (ou à
celui désigné par `KUBEWATCH_HELM_BIN`). Sans `helm` installé, la recherche
fonctionne mais le rendu échoue avec un message expliquant comment l'installer.
KubeWatch ne réimplémente pas le moteur de gabarits de Helm.

### Catalogue

Plus de vingt applications prêtes à déployer — bases de données, reverse
proxies, supervision, stockage objet, outils d'auto-hébergement. Chacune
pré-remplit le formulaire de déploiement avec des valeurs raisonnables.

### Déploiement

Un formulaire : image, ports, variables d'environnement, ressources demandées et
limites, volume persistant, Service, Ingress. Il produit un **YAML
multi-documents** que vous relisez — et que vous pouvez copier — avant de
l'envoyer.

« Déployer » l'applique au cluster ; la case *simulation* fait un `dry-run`
côté serveur. Rien n'est jamais appliqué sans que le manifeste ait été
affiché.

Ce formulaire expose tous les champs. Pour un déploiement ordinaire, l'[écran
Déployer](#écran-déployer) pose quatre questions et montre ce que le
déploiement coûtera au cluster ; le bouton « Assistant » y reprend le
formulaire en cours.

---

## Écran Mises à jour

`Ctrl+7` — cinq onglets.

| Onglet | Contenu |
| --- | --- |
| **Constats** | Les nouvelles versions détectées : surveillance, cible, version actuelle, version disponible, niveau (major / minor / patch). « Appliquer » déploie, après confirmation |
| **Surveillances** | Les surveillances configurées. Création, modification, suppression |
| **Historique** | Les déploiements et retours arrière effectués, le plus récent en tête : ancienne image, nouvelle, horodatage, statut |
| **Scan** | Les images réellement en service dans le cluster, et les surveillances **suggérées** à partir d'elles. Cochez celles qui vous intéressent, puis « Enregistrer la sélection » |
| **Réglages** | Jeton GitHub, canal par défaut, intervalle de vérification, déploiement automatique par défaut |

Le formulaire d'une surveillance reprend tout ce que décrit
[updates.md](updates.md) : source (release GitHub, tag de registre, chart Helm),
cible (namespace, type, nom, conteneur), canal semver, contrainte de version,
exclusions, fenêtre de maintenance, intervalle, déploiement automatique.

**Il n'y a pas d'ordonnanceur.** La vérification part du bouton de cet écran,
et de lui seul : l'application doit être ouverte — voir
[updates.md](updates.md#vérifications-régulières).

Le jeton GitHub saisi ici est stocké dans `updater.json` (permissions `0600`
sous Unix) et réaffiché masqué : le modifier ne demande pas de le retaper.

---

## Écran Réglages

`Ctrl+8` — deux parties.

**Les clusters.** Importer un kubeconfig (celui du système, ou un fichier
choisi), lister ses contextes et en importer un seul ou tous ; déclarer une
connexion distante par URL et jeton, avec un namespace par défaut ; choisir
d'enregistrer la connexion sur le disque ou de la garder en mémoire pour la
session ; retirer un cluster enregistré.

La case **« ignorer la vérification TLS »** est là pour les clusters de
laboratoire. Elle désactive réellement la vérification du certificat du
serveur : lisez
[security.md](security.md#ignorer-la-vérification-tls) avant de la cocher.

**L'application.** Thème clair ou sombre, facteur de zoom de l'interface,
période du rafraîchissement automatique, nombre d'objets demandés par page,
nombre de lignes de journal conservées en mémoire, et confirmation avant les
opérations destructrices.

Ces préférences sont conservées d'une session à l'autre, avec la géométrie de
la fenêtre et la largeur du panneau de navigation.

---

## Ce que l'interface ne fait pas

Honnêtement, et pour éviter de les chercher :

| Absent de l'interface | Où le faire |
| --- | --- |
| Redirection de port | `kubectl port-forward mon-pod 8080:80` |
| Sortie exploitable par un script, automatisation | `kubectl` : KubeWatch n'a pas de ligne de commande |
| Vérification planifiée des mises à jour | Nulle part : elle part du bouton de l'écran Mises à jour |
| Vérification de version de KubeWatch | La page des releases du dépôt |

L'interface n'a pas non plus de journal d'audit, d'éditeur de RBAC, ni de
comptes utilisateurs : elle agit sous votre identité, avec les droits de votre
kubeconfig. Voir [security.md](security.md).
