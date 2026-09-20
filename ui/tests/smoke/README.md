# Test de fumée des écrans

`npm test` monte chaque écran de l'interface dans un DOM simulé (jsdom), avec un
faux pont Tauri qui sert l'instantané anonymisé de `fixtures/snapshot.json`, et
échoue si un écran lève une erreur au rendu. Le vérificateur de types ne voit
pas les écarts entre les types déclarés et le JSON réel du backend (champs
omis quand ils sont vides, par exemple) : ce test, si.

Pour régénérer une fixture depuis un cluster réel :

```sh
cargo run -p kubewatch-core --example dump_graph -- [cluster] [namespace] > /tmp/snapshot.json
# puis anonymiser avant de la déposer ici : aucun nom réel ne doit entrer dans le dépôt.
```
