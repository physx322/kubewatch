## Résumé

<!-- Que fait cette PR, et pourquoi ? Deux ou trois phrases suffisent. -->

## Issue liée

<!-- « Closes #123 », « Refs #456 », ou « aucune » si la PR est autonome. -->

## Type de changement

<!-- Cochez ce qui s'applique. Le type doit correspondre au préfixe des commits. -->

- [ ] `fix` — correction de bug (sans rupture d'API)
- [ ] `feat` — nouvelle fonctionnalité (sans rupture d'API)
- [ ] `perf` — amélioration des performances ou de la taille du binaire
- [ ] `refactor` — réorganisation sans changement de comportement
- [ ] `docs` — documentation seule
- [ ] `test` — tests seuls
- [ ] `ci` / `build` — chaîne d'intégration, packaging, Dockerfile
- [ ] `security` — correction ou durcissement de sécurité
- [ ] Changement de rupture (`!` dans le sujet du commit **et** section ci-dessous remplie)

## Changement de rupture

<!--
À remplir uniquement si la case correspondante est cochée.
Décrivez ce qui casse (champ de DTO, format de l'état persistant, écran ou
raccourci) et la marche à suivre pour migrer.
-->

## Comment cela a été vérifié

<!--
Décrivez la vérification réelle : version et distribution de Kubernetes
(kind, k3s, EKS…), mode de connexion (kubeconfig, .yml, distant, in-cluster),
et ce qui a été observé.
-->

- Cluster de test :
- Commandes exécutées :
- Résultat observé :

## Checklist

- [ ] `make lint` passe (`cargo fmt --check` et clippy sans avertissement)
- [ ] `make test` passe
- [ ] Les commits suivent la convention [Conventional Commits](https://www.conventionalcommits.org/fr/v1.0.0/)
- [ ] Les nouveaux DTO exposés sur l'API HTTP dérivent `Serialize`/`Deserialize` avec `#[serde(rename_all = "camelCase")]`
- [ ] Aucune nouvelle dépendance externe, ou son ajout est justifié ci-dessous
- [ ] Aucun `unwrap()` sur une entrée externe, aucun `todo!()` ni `unimplemented!()`
- [ ] Les erreurs remontent via le type `Result` du crate concerné
- [ ] La documentation (README, docs/, commentaires) est à jour
- [ ] Aucun secret, jeton ni kubeconfig n'a été committé

## Impact sur la taille du binaire

<!--
La CI publie la taille du binaire release dans le résumé du job « build ».
Si cette PR ajoute une dépendance ou fait franchir un palier, indiquez l'avant
et l'après ici.
-->

## Notes pour la revue

<!-- Points d'attention, choix discutables, pistes écartées. -->
