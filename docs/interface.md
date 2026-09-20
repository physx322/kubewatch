# Guide de l'interface

Ce document décrit l'application de bureau `kubewatch-desktop`, écran par écran.
Le projet ne publie pas encore de captures : tout est décrit en texte, ce qui a
l'avantage de rester exact quand les couleurs changent.

- [La fenêtre](#la-fenêtre)
- [Raccourcis clavier](#raccourcis-clavier)
- [La barre latérale](#la-barre-latérale)
- [La barre supérieure](#la-barre-supérieure)
- [Écran Vue d'ensemble](#écran-vue-densemble)
- [Écran Ressources](#écran-ressources)
- [Le panneau de détail](#le-panneau-de-détail)
- [La console YAML](#la-console-yaml)
- [Écran Topologie](#écran-topologie)
- [Écran Hub](#écran-hub)
- [Écran Déployer](#écran-déployer)
- [Écran Mises à jour](#écran-mises-à-jour)
- [Écran Clusters](#écran-clusters)
- [Écran Réglages](#écran-réglages)
- [L'assistant IA](#lassistant-ia)
- [Ce que l'interface ne fait pas](#ce-que-linterface-ne-fait-pas)

---

## La fenêtre

La disposition ne change jamais : vous savez toujours où regarder.

```
┌──────────────┬──────────────────────────────────────────┬───────────────┐
│ KubeWatch    │ Ressources │ ● prod │ ▼ ns │ filtre… ⟳☀✦ │               │
│ ───────────  ├──────────────────────────────────────────┤   assistant   │
│ Vue d'ens. ⌃1│                                          │   (Ctrl+J)    │
│ Ressources ⌃2│                                          │               │
│ Topologie  ⌃3│                                          │  redimension- │
│ Hub        ⌃4│              zone centrale               │  nable à la   │
│ Déployer   ⌃5│         (l'écran sélectionné)            │  souris       │
│ Mises à j. ⌃6│                                          │               │
│ ───────────  │                                          │               │
│ Système      │                                          │               │
│ Clusters   ⌃7│                                          │               │
│ Réglages   ⌃,│                                          │               │
│ ───────────  │                                          │               │
│ v0.1.0       │                                          │               │
└──────────────┴──────────────────────────────────────────┴───────────────┘
```

Le panneau de l'assistant n'apparaît que si vous l'ouvrez ; sa largeur se règle
à la souris (entre 320 et 900 pixels) et elle est conservée d'une session à
l'autre. La barre latérale, elle, a une largeur fixe.

Deux principes valent partout :

- **Rien ne gèle.** Chaque opération réseau est une commande envoyée au cœur
  Rust, qui répond quand il a fini. Pendant qu'un cluster réfléchit — ou refuse
  de répondre pendant trente secondes — la fenêtre reste vivante : vous pouvez
  changer d'écran, ouvrir l'assistant, ou lancer autre chose.
- **L'application reste utilisable sans cluster joignable.** Aucun écran ne se
  vide, aucune boîte de dialogue ne bloque : un message décrit ce qui s'est
  passé et la vie continue.

Les messages transitoires — succès, erreurs, avertissements du démarrage —
apparaissent en **notifications** dans un coin, et disparaissent seules au bout
de cinq secondes (dix pour une erreur).

---

## Raccourcis clavier

| Raccourci | Effet |
| --- | --- |
| `Ctrl+1` … `Ctrl+7` | Changer d'écran, dans l'ordre de la barre latérale |
| `Ctrl+,` | Écran Réglages |
| `Ctrl+J` | Ouvrir ou fermer l'assistant IA |
| `Ctrl+K` | Placer le curseur dans le champ de filtre |
| `Ctrl+R` | Rafraîchir toutes les lectures de l'écran courant |
| `Échap` | Vider le champ de filtre ; fermer un dialogue, un menu contextuel, ou le panneau de détail de la topologie |

Sur macOS, `Ctrl` se lit **`Cmd`** : le modificateur employé est celui de la
plateforme. Les raccourcis à un chiffre ignorent `Maj` et `Alt`.

Dans le terminal du panneau de détail, les frappes partent au conteneur —
`Ctrl+C` interrompt le processus distant, `↑` et `↓` rappellent les commandes
précédentes. Deux exceptions restent locales : `Ctrl+Maj+C` copie la sélection
et `Ctrl+Maj+V` colle.

---

## La barre latérale

Six écrans en haut, puis une section **Système** avec deux entrées. Chaque
entrée porte une icône de la police [Phosphor](https://phosphoricons.com/) et
rappelle son raccourci.

| Entrée | Raccourci |
| --- | --- |
| Vue d'ensemble | `Ctrl+1` |
| Ressources | `Ctrl+2` |
| Topologie | `Ctrl+3` |
| Hub | `Ctrl+4` |
| Déployer | `Ctrl+5` |
| Mises à jour | `Ctrl+6` |
| Clusters | `Ctrl+7` |
| Réglages | `Ctrl+,` |

Tout en bas, la version du binaire.

---

## La barre supérieure

De gauche à droite :

| Élément | Rôle |
| --- | --- |
| **Titre** | Le nom de l'écran courant |
| **Sélecteur de cluster** | Les clusters enregistrés, précédés d'une pastille verte (connecté) ou rouge (injoignable). Un cluster hors ligne est suivi de « (hors ligne) ». L'adresse du serveur s'affiche au survol |
| **Sélecteur de namespace** | « Tous les namespaces », ou l'un d'eux. Le choix est retenu **par cluster** |
| **Champ de filtre** | Filtre **local** de la liste affichée. `Ctrl+K` y place le curseur, `Échap` le vide. Aucune requête réseau |
| **⟳** | Actualisation automatique : les listes toutes les 5 s, la synthèse toutes les 10 s, les évènements toutes les 15 s, la topologie toutes les 15 s. L'icône passe à la couleur d'accent quand elle est active |
| **☀ / 🌙 / ☼** | Thème : système → clair → sombre, en boucle. L'icône montre le thème courant |
| **✦** | Ouvre ou ferme l'assistant IA (`Ctrl+J`) |

Changer de cluster efface la sélection en cours et recharge l'écran ; changer de
namespace fait de même.

---

## Écran Vue d'ensemble

`Ctrl+1` — la synthèse du cluster courant.

En-tête : le nom du cluster, sa version, sa plate-forme, et un badge « hors
ligne » le cas échéant. Un bouton **« Actualiser »** relit la synthèse et les
évènements.

**Cinq compteurs** : nœuds prêts (sur le total), pods en cours, pods en attente,
pods en échec, namespaces. Les trois derniers passent en orange ou en rouge dès
qu'ils ne valent pas zéro.

**Deux jauges**, processeur et mémoire : le pourcentage occupé, la quantité
consommée sur la capacité, et une barre. Sans `metrics-server`, elles affichent
« metrics-server indisponible » et restent vides — le reste de l'écran
fonctionne.

**Charges de travail** : un bouton par type présent (Deployment, StatefulSet…)
avec son compte ; cliquer ouvre ce type dans l'écran Ressources.

**Évènements récents à surveiller** : les vingt-cinq derniers évènements de type
`Warning` du namespace choisi — quand, objet, raison, message, nombre
d'occurrences. Cliquer une ligne ouvre l'objet concerné dans Ressources.

Sans cluster sélectionné, l'écran se réduit à « Aucun cluster sélectionné » et à
un bouton **« Ajouter un cluster »** qui mène à l'écran Clusters.

---

## Écran Ressources

`Ctrl+2` — le tableau de n'importe quel type d'objet, et le panneau de détail à
côté.

**La barre d'outils** porte :

- le **sélecteur de type**, en deux groupes : « Courants » (pods, deployments,
  statefulsets, daemonsets, replicasets, jobs, cronjobs, services, ingresses,
  configmaps, secrets, persistentvolumeclaims, persistentvolumes, nodes,
  namespaces, events, serviceaccounts, horizontalpodautoscalers) puis « Tous les
  types », qui contient tout ce que l'API server expose — CRD de vos opérateurs
  comprises, sous la forme `pluriel.groupe` ;
- le **sélecteur de labels**, envoyé au serveur d'API (`app=web`) ;
- le nombre d'objets reçus, suivi de « (tronqué) » si la page est incomplète ;
- un bouton **« Console YAML »** ;
- un bouton d'actualisation.

**Le tableau** adapte ses colonnes au type : toujours le nom, le namespace
(quand tous les namespaces sont affichés), le statut, l'état « prêt » et l'âge ;
plus les redémarrages et le nœud pour les pods ; plus des colonnes propres à
chaque type — IP et QoS pour les pods, voulues/à jour/disponibles et stratégie
pour les deployments, type et IP de cluster et ports pour les services, classe
et hôtes et adresse et TLS pour les ingresses, rôles et version et runtime pour
les nœuds, capacité et classe et modes d'accès pour les volumes, et ainsi de
suite. Les images apparaissent pour les charges de travail.

Les **en-têtes sont cliquables** pour trier. Le tri et le filtre de la barre
supérieure s'appliquent localement, sur les lignes déjà reçues.

La liste est demandée par pages de 500 objets. Au-delà, elle est marquée
« tronqué » : affinez avec un sélecteur de labels ou un namespace. Il n'y a pas
de bouton « page suivante ».

Un **clic sur une ligne** ouvre le panneau de détail à droite ; un second clic
sur la même ligne le referme.

Quand la liste est vide, l'écran le dit plutôt que de laisser un tableau nu :
« Aucun objet », avec la précision du namespace concerné.

---

## Le panneau de détail

Il occupe la moitié droite de l'écran Ressources. En haut : le type de l'objet,
son namespace et son nom, son statut, son état « prêt », un bouton **« Actions »**
et une croix de fermeture.

### Les onglets

| Onglet | Contenu |
| --- | --- |
| **Résumé** | Nom, namespace, type et apiVersion, UID, date de création et âge, statut, prêt, redémarrages, nœud (cliquable), images, propriétaires (cliquables), et les colonnes supplémentaires du type. Puis les **labels**, et les **annotations** derrière un bouton « Afficher les annotations (n) » |
| **YAML** | Le manifeste complet, coloré syntaxiquement et **modifiable** |
| **Évènements** | Les évènements concernant cet objet : quand, type, raison, message, nombre. Relus toutes les 15 secondes |
| **Journaux** | Le journal d'un conteneur, en flux — pods uniquement |
| **Terminal** | Une session interactive dans un conteneur — pods uniquement |

Les propriétaires sont cliquables, ce qui permet de remonter d'un pod à son
ReplicaSet puis à son Deployment ; le nœud l'est aussi.

### Le menu Actions

Les entrées proposées dépendent du type de l'objet : ce qui n'a pas de sens
n'apparaît pas.

| Action | Ce qu'elle fait | Disponible pour |
| --- | --- | --- |
| Redimensionner… | Demande le nombre de répliques (0 à 1000) et l'applique. Zéro est signalé comme un arrêt | Deployment, StatefulSet, ReplicaSet |
| Changer l'image… | Demande le conteneur (vide = le conteneur unique) et la nouvelle image, pré-remplie avec l'actuelle | Charges de travail et pods |
| Redémarrage progressif | `rollout restart` | Deployment, StatefulSet, DaemonSet |
| Retour à la révision précédente | Revient à la révision précédente, après confirmation | Deployment, StatefulSet, DaemonSet |
| Mettre hors service (cordon) / Remettre en service (uncordon) | Bascule l'ordonnançabilité du nœud | Node |
| Vider le nœud (drain) | Met le nœud hors service et évince ses pods éligibles, après confirmation ; la liste des pods évincés est affichée | Node |
| Demander à l'assistant | Ouvre l'assistant sur une question déjà rédigée à propos de cet objet | Tout objet |
| Copier le nom | Dans le presse-papiers | Tout objet |
| Supprimer | Supprime l'objet, après confirmation, puis referme le panneau | Tout objet |

Les actions destructrices ouvrent une **confirmation** qui rappelle l'objet
visé, nom et namespace compris. Cette confirmation n'est pas désactivable.

### L'onglet YAML

Quatre boutons : **« Recharger »** (relit depuis le cluster et abandonne les
modifications), **« Copier »**, **« Appliquer »** et **« Remplacer »**. Les deux
derniers ne s'activent qu'après une modification, signalée par un badge
« modifié ».

La différence entre les deux compte :

- **Appliquer** fait un Server-Side Apply, comme `kubectl apply --server-side` :
  le serveur fusionne votre manifeste avec l'existant, champ par champ ;
- **Remplacer** écrase l'objet entier par le contenu de l'éditeur, comme
  `kubectl replace`. Une confirmation le rappelle avant d'agir.

### L'onglet Journaux

Le flux démarre **tout seul** dès qu'un conteneur est connu, et s'arrête quand
vous fermez le panneau.

La barre d'outils porte le sélecteur de conteneur (les conteneurs
d'initialisation sont préfixés `init:`), trois cases — **Suivre**,
**Horodatage**, **Instance précédente** —, le nombre de lignes initiales (100,
500, 2000 ou 10000), un champ **« Filtrer les lignes… »**, l'état du flux, puis
quatre boutons : pause ou relance, défilement automatique, copier, effacer.

Le filtre masque les lignes qui ne contiennent pas le texte saisi, sans
interrompre le flux. Le défilement automatique se décroche dès que vous remontez
dans l'historique, et se recolle quand vous redescendez au bas.

Le tampon est **borné à 20 000 lignes** : un conteneur bavard ne fait pas gonfler
la mémoire indéfiniment. Les lignes les plus anciennes sont perdues — pour un
historique complet, `kubectl logs … > fichier`. Côté cœur, les lignes sont
regroupées par lots avant d'être transmises, ce qui évite de saturer
l'application quand un pod écrit sans discontinuer.

### L'onglet Terminal

C'est **xterm.js**, un vrai émulateur : séquences ANSI, couleurs, applications
plein écran, redimensionnement suivi. La grille s'ajuste à la taille du panneau
et la nouvelle taille est transmise au conteneur.

La barre d'outils porte le sélecteur de conteneur, son état, et le choix de la
commande :

| Choix | Ce qui est lancé |
| --- | --- |
| **Interpréteur automatique** (par défaut) | Un petit script `sh` qui essaie plusieurs interpréteurs et retient le premier disponible — ce qui marche dans presque toutes les images, y compris les distributions minimales |
| **/bin/bash**, **/bin/sh** | Directement celui-là |
| **Commande personnalisée…** | Une ligne de commande que vous écrivez, découpée en respectant les guillemets |

**« Connecter »** ouvre la session, **« Déconnecter »** la ferme, et une flèche
la relance. À droite de l'état, la taille de la grille et, session ouverte,
« clavier actif » ou « cliquer pour saisir ». Deux boutons copient le contenu
(ou la sélection) et effacent l'écran.

Changer de conteneur, ou changer de pod, ferme la session en cours : aucune
session n'est laissée derrière.

---

## La console YAML

Elle s'ouvre en **dialogue**, depuis le bouton de l'écran Ressources ou depuis
un bloc YAML d'une réponse de l'assistant. Ce n'est pas un écran de la barre
latérale.

Un éditeur multi-documents occupe la moitié haute. En bas, à gauche : le
**namespace par défaut**, appliqué aux documents qui n'en déclarent pas — comme
`kubectl apply -n` —, et une case **« Forcer »**, qui reprend la propriété d'un
champ détenu par un autre gestionnaire. À n'utiliser qu'en connaissance de
cause : c'est ce qui permet d'écraser ce qu'un opérateur gère.

Quatre actions :

| Bouton | Effet |
| --- | --- |
| **Supprimer** | Supprime du cluster les ressources décrites par le manifeste, après confirmation |
| **Différences** | Compare le manifeste à l'état réel et affiche le diff, ressource par ressource |
| **Simuler** | Envoie le manifeste en `dry-run` côté serveur : il est validé, admission comprise, rien n'est écrit |
| **Appliquer** | Server-Side Apply, document par document, après confirmation |

Le compte rendu indique, **document par document**, ce qui a été créé,
configuré, laissé inchangé, simulé, supprimé ou refusé — avec le message du
serveur en cas d'échec.

Il n'y a pas d'historique des applications : le brouillon de l'éditeur survit à
la fermeture du dialogue, mais pas à celle de l'application.

---

## Écran Topologie

`Ctrl+3` — le graphe des objets du cluster courant et de leurs liens, pour
répondre d'un coup d'œil à « ce pod appartient à quoi, qui l'expose, où
tourne-t-il ? ».

Chaque objet est une carte : sa famille et son namespace en petit, son nom, le
compteur `prêts/total`, et une bande de couleur sur le bord gauche qui reprend
le statut — vert en marche, orange transitoire, rouge en erreur, gris inconnu.

**Ce qui est lu** : onze types, en parallèle et paginés — pods, replicasets,
deployments, statefulsets, daemonsets, jobs, cronjobs, services, ingresses,
persistentvolumeclaims, nodes. Un type refusé par les droits ou absent du
cluster devient un badge orange « n type(s) non lu(s) », sans empêcher le reste.

Les liens sont déduits de l'état réel, jamais devinés :

| Lien | De → vers | Source |
| --- | --- | --- |
| possède (trait plein) | Deployment → ReplicaSet → Pod ; StatefulSet, DaemonSet → Pod ; CronJob → Job → Pod | `metadata.ownerReferences` |
| sélectionne (tirets) | Service → Pod | le sélecteur du service confronté aux labels du pod |
| route vers (tirets) | Ingress → Service | les backends de l'ingress |
| planifié sur (pointillés) | Pod → Nœud | le nœud du pod |
| monte (pointillés) | Pod → PersistentVolumeClaim | les volumes du pod |
| utilise (pointillés) | Pod → ConfigMap, Secret | les références du pod |

**Se déplacer** : la molette zoome autour du pointeur (`Ctrl` double le pas),
glisser le fond déplace la vue, **« Ajuster »** cadre tout le graphe sans
dépasser l'échelle 1:1. En zoom arrière, les cartes ne gardent que leur nom,
puis seulement leur couleur de statut. Glisser une carte la déplace et
l'**épingle** : les dispositions ne la toucheront plus, jusqu'à
**« Réorganiser »** ou « Détacher » dans son menu.

**Disposition** — deux boutons :

- *Couches* (par défaut) — une colonne par famille, de gauche à droite :
  Ingress, Services, charges de travail, ReplicaSets et Jobs, Pods, volumes et
  configuration, nœuds. L'algorithme limite les croisements et aligne chaque
  carte sur ses voisines. Déterministe : deux lectures du même cluster donnent
  le même dessin.
- *Organique* — placement par forces : les objets liés se rapprochent, les
  autres se repoussent. Au-delà de **1 200 objets**, ce bouton est désactivé et
  la disposition retombe sur *Couches*.

**Couches** — huit cases : Ingress, Services, Charges, ReplicaSets, Pods,
Volumes, Config, Nœuds. Deux sont décochées par défaut : les **ReplicaSets** (le
Deployment est alors relié directement à ses pods ; un ReplicaSet sans
propriétaire reste visible, c'est lui la charge de travail) et la **Config**
(ConfigMaps et Secrets, qui encombrent vite). Deux cases complètent la ligne :
**« Masquer les isolés »**, qui ne garde que les objets reliés à au moins un
autre, et **« Légende »**.

**Lire** : survoler une carte met en avant son voisinage et atténue le reste ;
l'infobulle donne statut, prêt, redémarrages, nœud, âge et images. Le champ de
filtre de la barre supérieure fait la même chose pour tout ce qui correspond
(nom, namespace, statut, famille) et ses voisins directs. Un **clic** ouvre un
panneau latéral léger — identité, statut, âge, images, et la liste des liens,
chacun cliquable pour sauter à l'autre bout. Pour le YAML, les évènements, les
journaux ou le terminal, le bouton **« Ouvrir dans Ressources »** y mène. Le
**clic droit** ouvre un menu : « Ouvrir le détail », « Ouvrir dans Ressources »,
« Centrer la vue ici », « Épingler », « Copier le nom ».

**Ce qui manque se voit** : un objet cité par un autre mais absent de la lecture
— backend d'ingress qui ne pointe sur aucun service, nœud interdit par les
droits — apparaît quand même, en gris et marqué « non listé ». C'est souvent la
raison pour laquelle on vient ici. Les ConfigMaps et les Secrets ne sont jamais
listés : les cartes de la couche Config sont donc toujours dans cet état.

**Garde-fou** : au-delà de **2 000 objets**, rien n'est dessiné. L'écran propose
de réduire le périmètre — un namespace, moins de couches — ou d'**« Afficher
quand même »**.

La lecture suit le sélecteur de namespace et se rafraîchit comme les autres
écrans. Les cartes ne bougent pas au rafraîchissement : seul un objet nouveau
déclenche un placement.

---

## Écran Hub

`Ctrl+4` — trois onglets, et un bouton **« Écran Déployer »** à droite de la
barre d'onglets.

### Images

Recherche sur quatre registres, choisis dans un sélecteur :

| Registre | Recherche par mot-clé |
| --- | --- |
| **Docker Hub** | Oui |
| **GitHub Container Registry** | Non : saisissez `propriétaire/image` |
| **Quay** | Oui |
| **Registre OCI** | Non : saisissez la référence complète, `registre.local:5000/equipe/app` |

Les résultats s'affichent en tableau : image, description, étoiles,
téléchargements, date de mise à jour, registre ; les images officielles portent
un badge.

Choisir une image ouvre un panneau latéral en deux parties :

- **Tags** — la liste des tags, filtrable, triable par version, par date ou par
  nom, avec leur taille, leur date et leurs plateformes ;
- **Inspection** — une fois un tag choisi : empreinte (digest), plate-forme,
  ports exposés, entrypoint, commande, dépôt source, et deux blocs repliables
  pour les variables d'environnement et les étiquettes — lus directement dans le
  manifeste et la configuration OCI.

En bas du panneau, **« Déployer cette image »** pré-remplit l'écran Déployer et
y bascule.

### Charts Helm

Recherche de charts sur **Artifact Hub**. Chaque résultat donne le dépôt, la
version du chart, la version de l'application empaquetée, les étoiles, la
description, et des liens vers Artifact Hub, le site du projet et l'URL du dépôt
Helm.

C'est une **recherche, et rien d'autre** : KubeWatch n'appelle pas `helm`, ne
rend pas de chart et n'affiche pas de `values.yaml`. Pour installer un chart,
c'est `helm` dans votre terminal.

### Catalogue

Les applications prêtes à déployer, embarquées dans le binaire, regroupées par
catégorie : bases de données, caches, reverse proxies, supervision, stockage,
outils d'auto-hébergement. Chaque carte annonce son port, son besoin de volume,
son nombre de variables, son chart éventuel et un lien vers sa documentation.
**« Déployer »** pré-remplit l'écran Déployer avec des valeurs raisonnables.

---

## Écran Déployer

`Ctrl+5` — un assistant en quatre étapes, pour mettre une application sur le
cluster sans écrire de YAML ni connaître les champs d'un `Deployment`. Le fil
d'étapes en haut est cliquable : on revient en arrière à tout moment, on ne
saute pas une étape qu'on n'a pas encore atteinte.

### ① Quoi

Deux sources, au choix :

- **Catalogue** — la grille des applications embarquées, filtrable par texte et
  par catégorie. Un clic retient l'application et passe à l'étape suivante, en
  proposant déjà un profil de taille adapté à sa catégorie.
- **Mon image** — une référence d'image quelconque, vérifiée à la frappe : la
  forme canonique s'affiche en vert, ou le motif de rejet en clair.
  **« Lire l'image »** interroge le registre et rapporte les ports exposés, le
  nombre de variables d'environnement par défaut et la plate-forme, qui
  pré-remplissent le déploiement.

### ② Comment

Six blocs, tous pré-remplis avec des valeurs raisonnables :

| Bloc | Ce qu'on y règle |
| --- | --- |
| **Identité** | Nom (étiquette DNS-1123, vérifiée), namespace (les namespaces du cluster sont proposés), nombre de répliques — au curseur ou au clavier |
| **Taille** | Un profil parmi *Micro*, *Petit*, *Moyen* et *Grand* ; chaque carte annonce la demande et la limite qu'elle applique, et pour quel usage |
| **Accès** | *Interne au cluster*, *Port sur les nœuds*, *Adresse IP publique* ou *Nom de domaine* — traduits en type de Service et, pour le dernier, en Ingress avec domaine, classe et secret TLS. Puis la liste des ports écoutés |
| **Stockage** | Une case, une taille en Gio, un point de montage et une classe : c'est tout ce qu'un `PersistentVolumeClaim` demande ici |
| **Réglages de l'application** | Les variables d'environnement |
| **Étiquettes** | Des labels ajoutés à tous les objets créés, en plus de ceux que KubeWatch pose (`app.kubernetes.io/name`, `instance`, `managed-by`) |

Ce que l'assistant n'expose pas — commande et arguments, placement sur les
nœuds, secret de registre, compte de service, annotations, mode d'accès du
volume — garde sa valeur par défaut. Il n'y a pas de formulaire complet vers
lequel basculer.

### ③ Vérification

C'est l'étape qui distingue cet écran d'un formulaire :

- **Ce qui sera créé** — la liste des objets Kubernetes, nommés et décrits en
  clair : Deployment, Service, Ingress, PersistentVolumeClaim, et le namespace
  visé.
- **Consommation prévue** — pour le processeur et la mémoire, une barre montrant
  la part déjà consommée du cluster et, à sa suite, ce que ce déploiement
  réserve. Chiffres à l'appui : occupé, réservé, capacité, pourcentage après
  déploiement, place restante. Un verdict en tête dit si le déploiement **tient
  sans difficulté**, **passe mais charge le cluster** (au-delà de 85 %) ou
  **dépasse la capacité**.
- **À savoir** — les remarques que la prévision a levées, la plus grave en tête :
  capacité insuffisante, volume `ReadWriteOnce` partagé entre plusieurs
  répliques, image sur un tag mouvant, réplique unique, zéro réplique, aucun
  port déclaré, aucune classe d'Ingress précisée…
- **Manifeste Kubernetes** — le YAML multi-documents, repliable, copiable, en
  **lecture seule**. Rien n'est appliqué sans qu'il ait pu être lu.

L'empreinte additionnée est celle des **demandes** (`resources.requests`) du
profil multipliées par le nombre de répliques, car c'est sur elles que
l'ordonnanceur réserve la place. Le point de départ est la **consommation
mesurée** du cluster, qui vient de `metrics-server`. Comparer les deux donne un
ordre de grandeur, pas une simulation d'ordonnanceur : les réservations déjà
consenties aux autres applications n'apparaissent pas dans une mesure de
consommation, et l'écran le dit sous les barres. Sans `metrics-server`, les
barres disparaissent et le verdict devient « consommation du cluster inconnue » ;
le déploiement, lui, reste possible. Le volume demandé n'est comparé à aucune
capacité.

Si la validation locale échoue, le message précis s'affiche et les deux boutons
d'action sont désactivés : le manifeste n'est même pas demandé.

### ④ Résultat

Le bilan, document par document : objet, nom, action (créé, configuré,
inchangé, simulé, supprimé, échec) et message du serveur.

**« Simuler d'abord »** envoie les manifestes en `dry-run` côté serveur : ils
sont validés, rien n'est écrit, et **« Déployer pour de bon »** attend juste à
côté. Le vrai déploiement, lui, demande une confirmation qui rappelle le cluster
et le namespace visés. Après coup, **« Voir dans Ressources »** ouvre le
Deployment créé.

---

## Écran Mises à jour

`Ctrl+6` — quatre onglets. En-tête : **« Actualiser »** et **« Vérifier
maintenant »**, qui interroge toutes les sources amont des surveillants actifs.

| Onglet | Contenu |
| --- | --- |
| **Surveillants** | Les surveillants configurés : activé, nom, cluster, cible, conteneur, source, canal, dernier contrôle, dernière version connue. Par ligne : modifier, mettre en pause ou activer, supprimer |
| **Détections** | Les nouvelles versions détectées, en cartes : sévérité (majeure, mineure, correctif), cible, version actuelle → version disponible, date de publication, image complète, notes de version en Markdown, et **« Appliquer »**. Une case permet de réafficher les détections déjà appliquées |
| **Inventaire** | Les images réellement en service dans le cluster, par namespace, avec le dépôt source probable. **« Suggérer des surveillants »** en propose à partir d'elles : cochez, puis « Enregistrer la sélection » |
| **Historique** | Les deux cents derniers déploiements effectués par KubeWatch : date, cible, image précédente, nouvelle image, statut |

Les réglages du détecteur — jeton GitHub, canal par défaut, intervalle,
déploiement automatique — ne sont **pas** ici : ils vivent dans la section
« Mises à jour » de l'écran Réglages.

### Le formulaire d'un surveillant

Un dialogue en trois blocs :

- **Cible** — nom du surveillant, cluster, type d'objet (Deployment,
  StatefulSet, DaemonSet, CronJob), namespace, nom de l'objet, conteneur (vide =
  le premier du pod) ;
- **Source des versions** — *Release GitHub* (propriétaire, dépôt, préfixe de
  tag), *Registre d'images* (référence d'image) ou *Chart Helm* (dépôt, chart) ;
- **Politique** — canal (Majeure, Mineure, Correctif, Pré-version, Figée),
  contrainte semver, intervalle de vérification (60 s minimum), fenêtre de
  maintenance en UTC au format `HH:MM-HH:MM`, versions ignorées, et trois cases :
  accepter les pré-versions, appliquer automatiquement, surveillant actif.

Cocher l'application automatique affiche un avertissement : elle modifie le
déploiement sans confirmation, dans la fenêtre de maintenance si elle est
définie.

Le détail de chaque notion est dans [updates.md](updates.md).

### Les vérifications ne partent pas toutes seules

L'écran mentionne une « vérification en tâche de fond tant que l'application est
ouverte », et l'écran Réglages porte une case correspondante. Ce réglage est
enregistré, mais **aucun processus ne l'honore aujourd'hui** : une vérification
part du bouton « Vérifier maintenant », et de lui seul. Voir
[updates.md](updates.md#vérifications-régulières).

---

## Écran Clusters

`Ctrl+7` — le parc de clusters. En-tête : **« Actualiser »**, **« Serveur
distant »** et **« Importer un kubeconfig »**.

Le tableau donne, par cluster : l'état (pastille), le nom et son contexte, le
serveur, la version, le nombre de nœuds, le nombre de namespaces, le namespace
par défaut, la disponibilité des métriques, et la dernière erreur le cas
échéant. Par ligne : **« Utiliser »** (en faire le cluster courant), un bouton
**« Rafraîchir le catalogue des types »** — utile après l'installation d'une
CRD — et **« Retirer »**.

Retirer un cluster le supprime de KubeWatch et du fichier d'état ; le cluster
lui-même n'est pas touché, et vous pourrez le réimporter.

### Importer un kubeconfig

Le dialogue propose le chemin détecté (`$KUBECONFIG`, puis `~/.kube/config`), un
bouton **« Parcourir… »** qui ouvre le sélecteur de fichiers du système, et
**« Lire les contextes »**. La liste des contextes s'affiche alors, chacun avec
son cluster, son serveur et son namespace ; le contexte courant est coché par
défaut. **Chaque contexte coché devient un cluster KubeWatch.**

Si « Parcourir… » n'ouvre rien, c'est le portail XDG qui manque — le chemin
reste saisissable à la main. Voir
[installation.md](installation.md#bibliothèques-dexécution-sous-linux).

### Connexion à un serveur d'API

Pour un cluster sans kubeconfig : un nom dans KubeWatch, l'adresse du serveur,
un **jeton porteur** (ServiceAccount ou jeton OIDC déjà échangé) et
l'**autorité de certification** au format PEM. « Afficher les options
avancées » révèle le certificat et la clé cliente, le namespace par défaut, et
un proxy HTTP ou SOCKS5.

La case **« Ignorer la vérification TLS du serveur (déconseillé) »** est là pour
les clusters de laboratoire. Elle désactive réellement la vérification du
certificat du serveur, et l'affiche : lisez
[security.md](security.md#ignorer-la-vérification-tls) avant de la cocher.

**Tout cluster ajouté est écrit sur le disque**, dans `clusters.json`, jeton
compris. Il n'y a pas d'option « garder en mémoire pour cette session » : pour
un jeton de courte durée, retirez le cluster quand vous avez fini.

---

## Écran Réglages

`Ctrl+,` — quatre sections.

### Apparence et comportement

Deux préférences, et deux seulement : le **thème** (Système, Clair, Sombre) et
l'**actualisation automatique** (activée ou non ; les périodes ne sont pas
réglables). Elles sont conservées d'une session à l'autre, avec le cluster
courant, le namespace par cluster, l'écran ouvert et la largeur du panneau de
l'assistant.

### Assistant IA

Le tableau des fournisseurs : actif, nom, type, modèle, adresse, et l'état de la
clé (enregistrée, facultative, manquante). Cliquer une ligne la rend active.
**« Ajouter un fournisseur »** ouvre un dialogue avec cinq préréglages :

| Préréglage | Adresse par défaut |
| --- | --- |
| Claude (Anthropic) | `https://api.anthropic.com` |
| ChatGPT (OpenAI) | `https://api.openai.com/v1` |
| LM Studio (local) | `http://localhost:1234/v1` |
| Ollama (local) | `http://localhost:11434/v1` |
| Autre serveur compatible OpenAI | à renseigner |

**« Lister les modèles »** sert aussi de test de connexion : il rapporte le
nombre de modèles disponibles, ou l'erreur exacte du fournisseur. Le champ de
modèle devient alors une liste. Un profil accepte en plus une sortie maximale en
jetons, et — chez Anthropic — l'affichage du raisonnement.

Trois réglages généraux valent pour tous les profils : les **outils de lecture
du cluster** (activés ou non), le **nombre de tours d'outils par question** (1 à
50) et des **instructions supplémentaires**, ajoutées au message système.

Une clé enregistrée n'est jamais réaffichée : laisser le champ vide la conserve.

### Mises à jour

Les réglages du détecteur : jeton GitHub (masqué), canal par défaut, secret de
webhook GitHub (résiduel, plus rien ne le consomme), intervalle de vérification,
contrainte semver, et trois cases — vérification périodique en tâche de fond,
accepter les préversions, appliquer automatiquement. Les trois dernières valeurs
servent de **défaut aux nouveaux surveillants** ; chaque surveillant garde
ensuite les siennes.

### À propos

La version du binaire, le chemin du dossier d'état, et un lien vers le dépôt.

---

## L'assistant IA

`Ctrl+J`, ou l'icône ✦ de la barre supérieure : un panneau à droite,
redimensionnable à la souris.

Il **voit le contexte de l'écran** — cluster, namespace, objet sélectionné et
son YAML — et le rappelle en bas du panneau, sous forme d'étiquettes. Si les
outils sont activés, il peut en plus **lire le cluster lui-même** : synthèse,
namespaces, listes d'objets, manifestes, évènements, journaux de pods,
métriques. Ces sept outils sont **en lecture seule** : l'assistant propose un
manifeste ou une commande, c'est vous qui l'appliquez.

Chaque appel d'outil apparaît comme une carte dépliable, avec ses arguments et
son résultat, et une coche ou un avertissement selon l'issue. Les réponses sont
rendues en Markdown ; un bloc YAML porte un bouton **« Console YAML »** qui
l'ouvre dans la console, prêt à simuler ou à appliquer, et un bouton
**« Copier »**.

`Entrée` envoie, `Maj+Entrée` va à la ligne. Pendant une réponse, un bouton
l'interrompt. Une icône remet la conversation à zéro, une autre mène aux
réglages. En pied de panneau, les jetons lus et produits, et le modèle qui a
répondu.

Sans fournisseur configuré, le panneau le dit et renvoie aux Réglages. Les
conversations ne sont pas enregistrées : elles disparaissent à la fermeture.

Ce que l'assistant envoie à votre fournisseur est détaillé dans
[security.md](security.md#ce-qui-sort-de-votre-machine) — et c'est à lire avant
de le pointer sur un cluster sensible.

---

## Ce que l'interface ne fait pas

Honnêtement, et pour éviter de les chercher :

| Absent de l'interface | Où le faire |
| --- | --- |
| Redirection de port | `kubectl port-forward mon-pod 8080:80` |
| Rendu ou installation d'un chart Helm | `helm` : l'écran Hub ne fait que chercher sur Artifact Hub |
| Vérification planifiée des mises à jour | Nulle part : elle part du bouton de l'écran Mises à jour |
| Sortie exploitable par un script, automatisation | `kubectl` : KubeWatch n'a pas de ligne de commande |
| Vérification de version de KubeWatch | La page des releases du dépôt |
| Pagination au-delà de 500 objets par type | Affinez avec un namespace ou un sélecteur de labels |
| Réglage du zoom, de la période d'actualisation, de la taille des pages ou du tampon de journaux | Nulle part : ces valeurs sont fixées |

L'interface n'a pas non plus de journal d'audit, d'éditeur de RBAC, ni de
comptes utilisateurs : elle agit sous votre identité, avec les droits de votre
kubeconfig. Voir [security.md](security.md).
