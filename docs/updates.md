# Détecteur de mises à jour

KubeWatch compare ce qui **tourne** dans votre cluster à ce qui est **publié**
en amont, et peut déployer la nouvelle version quand vous l'y autorisez.

La comparaison part d'un geste : le bouton « Vérifier maintenant » de l'écran
Mises à jour. Aucun démon ne tourne en arrière-plan et il n'y a pas de ligne de
commande à planifier : la vérification a lieu quand l'application est ouverte
(voir [Vérifications régulières](#vérifications-régulières)).

- [Le principe](#le-principe)
- [Sources](#sources)
- [Normalisation des versions](#normalisation-des-versions)
- [Politique de mise à jour](#politique-de-mise-à-jour)
- [Canaux semver](#canaux-semver)
- [Contraintes et exclusions](#contraintes-et-exclusions)
- [Fenêtre de maintenance](#fenêtre-de-maintenance)
- [Déploiement automatique](#déploiement-automatique)
- [Retour arrière](#retour-arrière)
- [Vérifications régulières](#vérifications-régulières)
- [Le webhook GitHub a été retiré](#le-webhook-github-a-été-retiré)
- [Jeton GitHub et quotas](#jeton-github-et-quotas)
- [Exemple de bout en bout](#exemple-de-bout-en-bout)
- [Mise à jour de KubeWatch lui-même](#mise-à-jour-de-kubewatch-lui-même)

---

## Le principe

Une **surveillance** (*watcher*) associe une charge de travail de votre cluster à
une source de versions en amont :

```
   ┌──────────────────────┐          ┌────────────────────────┐
   │  Deployment          │          │  Source                │
   │  production/api      │  ◀────▶  │  github.com/exemple/api│
   │  conteneur « web »   │          │  (Releases)            │
   │  image :1.4.2        │          │  dernière : v1.5.1     │
   └──────────────────────┘          └────────────────────────┘
              │                                  │
              └──────────── politique ───────────┘
                      canal minor, >=1.4 <2
                                 │
                                 ▼
                    détection : 1.4.2 → 1.5.1 (minor)
                                 │
              auto-apply ? ──non──▶ affichée, en attente de votre clic
                     │
                    oui
                     │
                     ▼
        set-image + suivi du rollout + entrée d'historique
```

À chaque vérification, KubeWatch :

1. lit l'objet ciblé dans le cluster et en extrait l'image du conteneur
   surveillé ;
2. en déduit la version courante à partir de l'étiquette de l'image ;
3. interroge la source pour obtenir les versions candidates ;
4. laisse la politique trancher : une candidate est retenue, ou aucune ;
5. enregistre une **détection** (*finding*), et l'applique si — et seulement
   si — vous l'avez autorisé.

---

## Sources

| Source | `type` de l'API | Ce qui est interrogé |
| --- | --- | --- |
| **GitHub Releases** | `githubRelease` | Les releases publiées du dépôt : tag, notes, date, statut préversion/brouillon |
| **Registre de conteneurs** | `containerRegistry` | La liste des tags de l'image (Docker Hub, GHCR, Quay, registre OCI générique) |
| **Chart Helm** | `helmChart` | Les versions publiées du chart |

Le formulaire d'une surveillance, dans l'écran Mises à jour, propose les trois
sources. Par exemple :

| Source | Cible | Réglages |
| --- | --- | --- |
| GitHub Releases | Deployment `api`, conteneur `web` | dépôt `exemple/api`, préfixe d'étiquette `v`, canal `minor` |
| Registre de conteneurs | Deployment `api` | image `ghcr.io/exemple/api`, canal `patch` |
| Chart Helm | StatefulSet `postgres` | chart `bitnami/postgresql`, canal `minor` |

Le champ dépôt accepte `owner/repo` comme une URL complète
(`https://github.com/owner/repo`, avec ou sans `.git`).

Les brouillons sont toujours écartés. Les préversions ne sont retenues que si
la politique les autorise.

---

## Normalisation des versions

Les étiquettes du monde réel ne sont pas du semver strict. KubeWatch les ramène
à une version comparable :

| Étiquette | Interprétation |
| --- | --- |
| `1.4.2` | `1.4.2` |
| `v1.4.2` | `1.4.2` (préfixe `v` retiré) |
| `release-1.4.2` | `1.4.2` (préfixe alphabétique connu retiré) |
| `1.4` | `1.4.0` (complétée) |
| `1.4.2-alpine` | `1.4.2-alpine` — préversion au sens semver, donc écartée hors canal `prerelease` |
| `1.4.2+build.7` | `1.4.2` (métadonnées de build conservées, ignorées pour la comparaison) |
| `sha256:9f2c…` | **ignorée** (empreinte) |
| `a1b2c3d4e5f6…` | **ignorée** (hexadécimal, probable SHA de commit) |
| `2026.09.18`, `20260918` | **ignorée** (datée) |
| `latest`, `stable`, `edge`, `main` | **ignorée** (mouvante) |

Le **préfixe d'étiquette** retire un préfixe supplémentaire, spécifique au
projet surveillé, **avant** ce traitement : avec le préfixe `api-`, `api-1.4.2`
est compris.

Un projet dont les étiquettes ne sont pas versionnées (par exemple un `latest`
reconstruit en continu) ne donne aucune candidate : c'est normal, et c'est
visible dans les traces avec `RUST_LOG=debug`.

---

## Politique de mise à jour

Chaque surveillance porte sa politique. Les valeurs par défaut sont
délibérément prudentes :

| Champ | Défaut | Rôle |
| --- | --- | --- |
| `channel` | `patch` | Amplitude du saut autorisé |
| `allowPrerelease` | `false` | Accepte alpha, beta, rc |
| `constraint` | — | Contrainte semver supplémentaire |
| `ignore` | `[]` | Versions exclues nommément |
| `autoApply` | `false` | Déployer sans intervention |
| `checkIntervalSeconds` | `3600` | Période de vérification (plancher effectif : 60 s) |
| `maintenanceWindow` | — | Fenêtre UTC `HH:MM-HH:MM` |

Une politique par défaut, appliquée aux nouvelles surveillances, se règle une
fois pour toutes dans l'écran Réglages, section « Mises à jour » : canal,
intervalle, contrainte semver, préversions et déploiement automatique.

---

## Canaux semver

Soit `1.4.2` la version courante :

| Canal | Retenu | Écarté |
| --- | --- | --- |
| `patch` | `1.4.3`, `1.4.9` | `1.5.0`, `2.0.0` |
| `minor` | `1.4.3`, `1.5.0`, `1.9.9` | `2.0.0` |
| `major` | `1.4.3`, `1.5.0`, `2.0.0` | — |
| `prerelease` | comme `major`, préversions comprises (`1.5.0-rc.1`) | — |
| `pinned` | **rien** | tout |

`pinned` fige la version : la surveillance continue d'exister et de rapporter
l'état, mais ne proposera jamais rien. C'est le moyen propre de geler un
composant sans supprimer sa surveillance.

Parmi les candidates autorisées, c'est toujours **la plus élevée** qui est
retenue — pas la suivante immédiate.

Le champ `severity` d'une détection (`major`, `minor`, `patch`, `unknown`)
décrit l'ampleur du saut réellement proposé : il sert à trier visuellement,
indépendamment du canal configuré.

---

## Contraintes et exclusions

`constraint` est une contrainte semver classique, évaluée **en plus** du canal.
Par exemple, le canal `major` avec la contrainte `>=1.4, <2` : ici le canal
autorise les majeures, mais la contrainte plafonne à la branche 1.x :
la 2.0.0 ne sera jamais proposée.

Formes acceptées : `>=1.2`, `<2`, `~1.4`, `^1.4.2`, `1.4.*`, combinables par
virgules. **Une contrainte invalide est ignorée** — avec un avertissement dans
les traces — plutôt que de bloquer silencieusement toutes les mises à jour.

Le champ **exclusions** écarte des versions précises, par exemple une release
connue pour être cassée : `1.5.0`, `1.5.1`.

---

## Fenêtre de maintenance

Format : `HH:MM-HH:MM`, en **UTC**.

| Fenêtre | Sens |
| --- | --- |
| `02:00-04:00` | entre 2 h et 4 h UTC |
| `22:00-06:00` | à cheval sur minuit : de 22 h au lendemain 6 h |

La fenêtre ne conditionne que le **déploiement automatique**. La détection, elle,
a lieu à chaque vérification, quelle que soit l'heure : la mise à jour
disponible s'affiche tout de suite, elle n'est simplement posée qu'au bon
moment.

Corollaire à ne pas manquer : le déploiement automatique ne peut se produire
que **pendant une vérification**, donc pendant que l'application est ouverte et
que vous en lancez une. Une fenêtre de maintenance à 3 h du matin ne servira
jamais si vous ne vérifiez qu'à midi.

Sans fenêtre, le déploiement automatique peut survenir à tout instant. Une
fenêtre illisible est ignorée (avec un avertissement) : la surveillance n'est
jamais bloquée par une faute de frappe.

Pensez au décalage horaire : `02:00-04:00` UTC, c'est 3 h – 5 h en hiver et
4 h – 6 h en été pour la France métropolitaine.

---

## Déploiement automatique

Avec `autoApply`, une détection éligible est déployée sans intervention. Quatre
conditions doivent être réunies simultanément :

1. une vérification est en cours (bouton « Vérifier maintenant » de l'écran
   Mises à jour) ;
2. la surveillance est **activée** et sa politique a `autoApply: true` ;
3. l'instant présent est **dans la fenêtre de maintenance** (ou il n'y en a
   pas) ;
4. la détection n'a pas déjà été appliquée.

Le déploiement lui-même :

1. remplace l'image du conteneur surveillé (`set-image`, patch de l'objet) ;
2. suit le rollout jusqu'à ce que les réplicas soient à jour et disponibles ;
3. écrit une entrée d'historique (`RolloutResult`) avec l'ancienne image, la
   nouvelle, l'horodatage et le statut.

Les déploiements automatiques sont **séquentiels** : deux rollouts simultanés sur
le même cluster rendent les diagnostics illisibles. Si l'un échoue, il est
journalisé et les suivants continuent.

Dans l'écran Mises à jour : « Vérifier maintenant » détecte seulement ; le
bouton « Appliquer » d'une détection la déploie ; l'onglet Historique liste les
déploiements, réussis ou non.

> `autoApply` sur un Deployment de production, sans fenêtre de maintenance et
> sans sonde de readiness fiable, c'est un redémarrage surprise en pleine
> journée. Commencez par `autoApply: false` : la détection seule apporte déjà
> l'essentiel.

---

## Retour arrière

Le déploiement passe par le mécanisme standard de Kubernetes : l'historique des
révisions reste disponible. Dans l'écran Ressources, le menu contextuel de la
charge de travail propose « Revenir à la révision précédente », l'équivalent de
`kubectl rollout undo`.

Pensez à figer la surveillance sitôt après, faute de quoi la prochaine
vérification reproposera la version fautive : passez-la sur le canal `pinned`,
ou, plus ciblé, ajoutez la version fautive à ses exclusions.

---

## Vérifications régulières

**Il n'y a ni démon, ni tâche planifiée.** L'ordonnanceur tournait dans le
serveur HTTP, retiré avec le pivot vers l'application de bureau ; la ligne de
commande qui permettait de confier la vérification à systemd ou cron a été
retirée à son tour. Une vérification part donc d'un seul geste : le bouton
« Vérifier maintenant » de l'écran Mises à jour, l'application étant ouverte.

L'intervalle enregistré sur chaque surveillance, et le réglage
`schedulerEnabled` hérité du mode serveur, sont conservés dans le fichier d'état
mais aucun processus ne tourne pour les honorer : c'est vous qui donnez le
rythme.

Pour une vérification automatique hors de l'application, c'est votre chaîne CI
qui doit s'en charger, avec `kubectl set image` depuis un runner qui a accès au
cluster. Le déploiement automatique de KubeWatch, lui, ne s'applique que pendant
une vérification lancée depuis l'écran.

---

## Le webhook GitHub a été retiré

Jusqu'à la version précédente, GitHub pouvait prévenir KubeWatch d'une
publication en appelant `POST /api/webhooks/github`, ce qui rendait la détection
immédiate. **Cette route n'existe plus** : il n'y a plus de serveur HTTP pour la
recevoir, et une application de bureau derrière un pare-feu domestique n'est de
toute façon pas joignable depuis GitHub.

Concrètement :

- Le secret de webhook encore présent dans `updater.json` n'est jamais renvoyé
  en clair, et plus rien ne le consomme : le module de vérification HMAC-SHA256
  subsiste dans la bibliothèque `kubewatch-updater`, mais aucun binaire
  n'expose d'endpoint pour l'appeler. Il se modifie depuis l'onglet Réglages de
  l'écran Mises à jour.
- Si vous aviez déclaré un webhook côté GitHub, supprimez-le : ses livraisons
  partent désormais dans le vide.
- Pour rapprocher la détection de la publication, vérifiez plus souvent. Avec
  un jeton GitHub, le quota de 5000 requêtes par heure laisse beaucoup de
  marge.
- Si vous avez besoin d'une réaction **immédiate** à une publication, c'est
  votre chaîne CI qui doit la déclencher : faites-lui appeler
  `kubectl set image` depuis un runner qui a accès au cluster. C'est plus
  direct, et cela n'expose aucun port.

---

## Jeton GitHub et quotas

L'API GitHub anonyme est limitée à **60 requêtes par heure et par adresse IP**.
Avec quelques surveillances et une vérification horaire, la limite est atteinte
vite, et l'API répond `429 rateLimited`.

Un jeton relève la limite à **5000 requêtes par heure**. Un jeton
« fine-grained » **sans aucune permission** suffit : KubeWatch ne lit que des
données publiques.

Le jeton se saisit dans l'onglet Réglages de l'écran Mises à jour. Il est
stocké dans `updater.json` (permissions `0600` sous Unix) et masqué partout où
il est réaffiché.

---

## Exemple de bout en bout

Objectif : surveiller le Deployment `api` du namespace `production`, dont
l'image est `ghcr.io/exemple/api:1.4.2`, contre les releases GitHub du dépôt
`exemple/api`. Déploiement automatique des mineures, la nuit uniquement.

Tout se passe dans l'écran Mises à jour :

1. **Point de départ : que tourne-t-il réellement ?** Onglet Inventaire,
   « Inventorier » : chaque conteneur en service apparaît avec son image
   (`ghcr.io/exemple/api:1.4.2`).
2. **Proposition automatique.** « Suggérer » propose une surveillance par image
   versionnée ; relisez, décochez ce qui ne vous intéresse pas, enregistrez.
3. **Ou déclaration explicite**, avec la politique voulue : cible `api`,
   Deployment, conteneur `web` ; source GitHub `exemple/api`, préfixe `v` ;
   canal `minor`, contrainte `>=1.4, <2` ; déploiement automatique, fenêtre
   `02:00-04:00`.
4. **Jeton GitHub**, onglet Réglages, pour ne pas se heurter aux 60 requêtes
   par heure.
5. **Première vérification**, « Vérifier maintenant » : la détection
   `1.4.2 → 1.5.1 (minor)` apparaît dans l'onglet Détections.
6. **Déploiement immédiat**, sans attendre la fenêtre : « Appliquer » sur la
   détection. L'onglet Historique enregistre `1.4.2 → 1.5.1` avec son statut.
7. **Vérification** : l'écran Ressources montre la nouvelle image sur le
   Deployment.
8. **En cas de problème** : « Revenir à la révision précédente » depuis le menu
   contextuel du Deployment, puis ajoutez `1.5.1` aux exclusions de la
   surveillance.

Le même parcours se fait à la souris dans l'application de bureau, écran
Mises à jour — voir [interface.md](interface.md#écran-mises-à-jour).

---

## Mise à jour de KubeWatch lui-même

L'application ne vérifie pas sa propre version et ne se met jamais à jour
d'elle-même. Suivez la page des releases du dépôt, et remplacez le binaire selon
la procédure décrite dans [installation.md](installation.md#mise-à-jour) — la
seule qui passe par une vérification de signature.
