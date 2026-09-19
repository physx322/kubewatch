# Contribuer à KubeWatch

Merci de l'intérêt porté au projet. Ce document décrit ce qu'il faut savoir
avant d'ouvrir une issue ou une Pull Request : l'outillage, les conventions, le
processus de release et les critères de revue.

Toute contribution est publiée sous licence [Apache 2.0](LICENSE).

---

## 1. Ligne directrice du projet

KubeWatch tient en **un seul binaire, léger, sans dépendance système**, qui
fonctionne à distance, depuis un fichier `.yml` ou depuis l'intérieur du
cluster. Trois conséquences pratiques pour toute contribution :

1. **Chaque dépendance se justifie.** Le graphe de dépendances est figé et
   résolu ; en ajouter une doit apporter davantage que le poids, la surface
   d'attaque et le temps de compilation qu'elle coûte.
2. **La taille du binaire est un budget, pas une statistique.** La CI publie la
   taille à chaque build et échoue au-delà du budget fixé.
3. **Pas de dépendance à un outil externe pour les fonctions de base.** `helm`
   est utilisé s'il est présent, jamais exigé ; tout le reste passe par
   l'API Kubernetes.

---

## 2. Prérequis

| Outil | Version | Rôle |
| --- | --- | --- |
| Rust | 1.85 minimum (MSRV), stable récent recommandé | compilation |
| `cargo` | fourni avec Rust | build, tests, lint |
| `rustfmt`, `clippy` | composants rustup | formatage et lint |
| Git | 2.30+ | historique et tags |
| Docker (optionnel) | 24+ avec Buildx | image conteneur |
| Un cluster de test (optionnel) | `kind`, `k3d`, `minikube` | tests manuels |

Outils d'audit installables à la demande :

```sh
cargo install cargo-audit cargo-deny git-cliff
```

### Toolchain : si `rustup` est cassé

Sur certaines installations, le shim `rustup` ne résout pas `cargo`. Exportez
alors directement le chemin de la toolchain :

```sh
export PATH=~/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH
```

Le `Makefile` accepte aussi une surcharge ponctuelle, sans rien exporter :

```sh
make build CARGO=~/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo
```

---

## 3. Commandes de développement

Tout passe par le `Makefile` (`make` seul affiche l'aide) :

```sh
make build      # compilation debug du workspace
make run        # lance l'application de bureau
make dev        # idem, recompilée et relancée à chaque sauvegarde (cargo-watch)
make test       # tests du workspace, toutes features
make lint       # cargo fmt --check puis clippy -D warnings
make fmt        # formatage
make doc        # documentation, liens cassés bloquants
make audit      # vulnérabilités RustSec
make deny       # licences, sources, interdictions
make packaging-check  # validation de l'entrée .desktop et du fichier AppStream
make release    # binaire optimisé (profil dist)
make dist       # archive .tar.gz + .sha256 dans dist/
make ci         # enchaîne lint, test, release, packaging-check, audit
make clean      # nettoyage
```

Quelques variables utiles : `TARGET=aarch64-unknown-linux-gnu make release`,
`PREFIX=/usr/local make desktop-install`, `RUST_LOG=debug make run`.

### Structure du dépôt

| Chemin | Contenu |
| --- | --- |
| `crates/core` | accès Kubernetes : connexion, découverte, ressources, apply, logs, exec, port-forward, métriques, évènements |
| `crates/hub` | registres d'images, charts Helm, catalogue d'applications, génération de manifestes |
| `crates/updater` | détection d'updates GitHub et registre, politiques SemVer, rollouts, webhooks |
| `crates/desktop` | le binaire `kubewatch-desktop` : application egui/eframe, pont asynchrone, écrans, widgets |
| `packaging/` | entrée de menu freedesktop, métadonnées AppStream, recettes d'empaquetage |
| `docs/` | documentation |

### Règles de code

- Commentaires et messages destinés à l'utilisateur **en français** ;
  identifiants, types et noms de fichiers **en anglais**.
- Pas de `todo!()`, pas de `unimplemented!()`, pas de `unwrap()` sur une entrée
  externe (réponse d'API, YAML fourni, paramètre de requête).
- Les erreurs remontent par le type `Result` du crate concerné et portent un
  code HTTP cohérent via `status_code()`.
- Tout DTO exposé sur l'API HTTP dérive
  `#[derive(Debug, Clone, Serialize, Deserialize)]` et porte
  `#[serde(rename_all = "camelCase")]` : l'interface web en dépend.
- Les routes de l'API HTTP forment un contrat. En modifier une est un
  changement de rupture.

---

## 4. Convention de commits

Le projet suit [Conventional Commits](https://www.conventionalcommits.org/fr/v1.0.0/).
Les messages ne sont pas cosmétiques : `git cliff` en déduit le CHANGELOG, les
notes de release et le calcul de la prochaine version.

```
<type>(<portée>)<!>: <sujet à l'impératif, sans majuscule initiale, sans point final>

<corps facultatif : le pourquoi plutôt que le comment>

<pied facultatif : BREAKING CHANGE: …, Closes #123>
```

Types acceptés et section correspondante du changelog :

| Type | Section | Effet sur la version |
| --- | --- | --- |
| `feat` | Nouveautés | mineure |
| `fix` | Corrections | correctif |
| `security` | Sécurité | correctif |
| `perf` | Performance | correctif |
| `refactor` | Refactorisation | correctif |
| `docs` | Documentation | correctif |
| `test` | Tests | correctif |
| `ci`, `build` | Intégration continue | correctif |
| `chore` | Divers | correctif |

Portées usuelles : `core`, `hub`, `updater`, `app`, `ui`, `api`, `cli`,
`docker`, `ci`, `deps`.

Un changement de rupture se signale par un `!` après la portée **ou** par un
pied `BREAKING CHANGE:` ; il provoque une version majeure.

```
feat(hub): ajouter la recherche d'images sur Quay

fix(core): ne plus paniquer quand metrics-server est absent

feat(api)!: renommer /api/clusters/{name}/objects en /resources

BREAKING CHANGE: l'ancienne route est supprimée, l'UI doit être mise à jour.
```

Les commits `chore(release)` sont exclus du changelog : ils appartiennent à
l'outillage.

---

## 5. Cycle d'une Pull Request

1. **Ouvrir une issue d'abord** pour tout changement structurant (nouvelle
   dépendance, nouveau sous-système, modification de l'API HTTP). Une
   correction de bug évidente peut aller directement en PR.
2. **Brancher depuis `main`** : `fix/…`, `feat/…`, `docs/…`.
3. **Vérifier localement** : `make lint && make test`. La CI refait tout, mais
   elle est plus lente que votre machine.
4. **Remplir le gabarit de PR**, en particulier la façon dont le changement a
   été vérifié (version de Kubernetes, mode de connexion, observations).
5. **La CI doit être verte** : formatage, clippy sans avertissement, tests sur
   Linux/macOS/Windows, MSRV 1.85, budget de taille, documentation, audit de
   sécurité et de licences.
6. **Une revue d'approbation** est requise avant fusion.
7. **Fusion en squash**, avec un message de commit conventionnel : c'est lui
   qui apparaîtra dans le changelog.

### Ce qui est regardé en revue

- Le comportement en cas d'échec : cluster injoignable, RBAC insuffisant,
  `metrics-server` absent, CRD inconnue, YAML invalide. Une erreur explicite
  vaut mieux qu'un écran vide.
- L'absence de `unwrap()` sur des données externes et la propagation correcte
  des erreurs.
- Le respect du contrat d'API (routes, noms de champs en camelCase).
- L'impact sur la taille du binaire et le temps de compilation.
- La couverture par des tests des parties purement logiques (parsing de
  quantités, sélection SemVer, fenêtres de maintenance, rendu de manifestes) —
  elles se testent sans cluster, il n'y a pas de raison de s'en priver.
- Le traitement des secrets : jamais journalisés, jamais renvoyés par l'API,
  fichier d'état en `0600`.

---

## 6. Processus de release

Le versioning est sémantique et entièrement piloté par les tags.

1. **Préparer la version.** `release-plz` (configuré dans `release-plz.toml`)
   calcule la prochaine version à partir des commits, met à jour les
   `Cargo.toml` du workspace et le `CHANGELOG.md`, puis ouvre une PR de
   release. Les quatre crates partagent une version unique.
2. **Fusionner la PR de release.**
3. **Le tag est posé automatiquement.** À chaque push sur `master`, le job
   `autotag` de `release.yml` compare la version du workspace aux tags
   existants et pose `vX.Y.Z` s'il manque — à condition que
   `packaging/io.kubewatch.KubeWatch.metainfo.xml` annonce cette version, ce
   qui distingue une release voulue d'un simple bump en cours de développement.
   Sans cette entrée, le job s'arrête avec un avertissement et rien n'est
   publié.

   Poser le tag à la main reste possible, par exemple pour republier :

   ```sh
   git switch master && git pull
   git tag -a v0.2.0 -m "KubeWatch 0.2.0"
   git push origin v0.2.0
   ```

   Le tag doit être strictement égal à `v` + la version du workspace :
   `release.yml` refuse de publier en cas d'écart.

4. **`release.yml` prend le relais** automatiquement :
   - notes de version générées par `git-cliff` et release GitHub en brouillon ;
   - compilation des cinq cibles (Linux glibc et macOS en x86_64 et aarch64,
     Windows x86_64) au profil `dist` ;
   - archives `.tar.gz` / `.zip` contenant le binaire, le README et la licence,
     avec un `.sha256` par archive ;
   - agrégation en `SHA256SUMS`, signé sans clé par cosign (identité OIDC du
     workflow) ;
   - bascule de la release en publiée ; publication crates.io uniquement si la
     variable de dépôt `PUBLISH_CRATES` vaut `true`.
Une préversion (`v1.0.0-rc.1`) est publiée comme *pre-release* et ne devient
jamais `latest`.

### Vérifier une release publiée

```sh
sha256sum -c SHA256SUMS --ignore-missing

cosign verify-blob SHA256SUMS \
  --signature SHA256SUMS.sig \
  --certificate SHA256SUMS.pem \
  --certificate-identity-regexp "^https://github.com/kubewatch-io/kubewatch/" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

---

## 7. Signaler une faille de sécurité

**N'ouvrez pas d'issue publique.** Utilisez l'onglet *Security* du dépôt
(« Report a vulnerability ») pour un signalement privé. Merci d'indiquer la
version concernée, l'impact et une reproduction minimale ; une première réponse
est visée sous 72 heures.

---

## 8. Intégration continue

| Workflow | Déclencheur | Rôle |
| --- | --- | --- |
| `ci.yml` | push, PR | formatage, clippy, tests 3 OS, MSRV, budget de taille, documentation |
| `security.yml` | push, PR, lundi 05:17 UTC | RustSec, cargo-deny, cargo-vet (informatif), CodeQL |
| `release.yml` | tag `v*.*.*` | binaires, sommes de contrôle signées, release |
| `docker.yml` | `main`, tags `v*` | image multi-arch signée sur ghcr.io |

Dependabot propose chaque lundi les montées de version des crates, des actions
et de l'image de base ; ces PR passent par la même CI que les autres.
