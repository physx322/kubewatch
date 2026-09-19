//! Commandes envoyées par l'interface au worker asynchrone.
//!
//! L'interface (egui, synchrone et *immediate-mode*) ne fait jamais d'appel réseau :
//! elle pousse une [`Command`] dans une file non bornée et poursuit son rendu. Le
//! worker, qui tourne dans le runtime tokio, exécute la commande puis renvoie un
//! ou plusieurs [`crate::backend::Event`].
//!
//! Chaque commande longue porte un [`RequestId`] monotone, repris tel quel dans les
//! évènements correspondants : l'interface peut ainsi ignorer les réponses obsolètes
//! lorsque l'utilisateur a changé d'écran entre-temps.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use kubewatch_core::logs::LogOptions;
use kubewatch_core::model::{ListOptions, ResourceRef};
use kubewatch_core::ConnectionSpec;
use kubewatch_hub::deploy::DeployRequest;
use kubewatch_hub::model::RegistryKind;
use kubewatch_updater::model::WatcherSpec;
use kubewatch_updater::store::UpdaterSettings;

/// Identifiant d'une requête en vol.
///
/// Monotone et strictement croissant ; il est attribué par
/// `AppState::next_id()` côté interface.
pub type RequestId = u64;

/// Identifiant réservé aux évènements spontanés du worker.
///
/// Le worker l'utilise pour les messages qu'aucune requête de l'interface n'a
/// demandés : fin du chargement des clusters persistés au démarrage, rafraîchissement
/// consécutif à une sélection ou à une suppression de cluster. `AppState::next_id()`
/// doit donc commencer à 1 afin de ne jamais le réutiliser.
pub const NO_REQUEST: RequestId = 0;

/// Ordre adressé au worker asynchrone.
///
/// Les variantes sans `id` (`RemoveCluster`, `SelectCluster`, `Quit`) sont des
/// commandes d'ordre : le worker les traite en ligne, sans les confier à une tâche,
/// afin qu'elles prennent effet immédiatement même si des commandes lentes sont en
/// cours. `StopLogs` et `StopExec` réutilisent l'`id` du flux à interrompre.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum Command {
    // ------------------------------------------------------------- clusters
    /// Connecte un cluster et l'enregistre sous `name`.
    Connect {
        /// Identifiant de la requête.
        id: RequestId,
        /// Manière de joindre le serveur d'API.
        spec: ConnectionSpec,
        /// Nom local donné au cluster dans KubeWatch.
        name: String,
        /// Écrit la spécification dans le fichier d'état.
        persist: bool,
    },
    /// Importe un kubeconfig : un cluster par contexte, ou le seul contexte courant.
    ImportKubeconfig {
        /// Identifiant de la requête.
        id: RequestId,
        /// Chemin du fichier ; `None` = `$KUBECONFIG` puis `~/.kube/config`.
        path: Option<PathBuf>,
        /// Importe tous les contextes plutôt que le seul `current-context`.
        all_contexts: bool,
    },
    /// Liste les contextes déclarés par un kubeconfig, sans s'y connecter.
    ListContexts {
        /// Identifiant de la requête.
        id: RequestId,
        /// Chemin du fichier ; `None` = kubeconfig par défaut.
        path: Option<PathBuf>,
    },
    /// Retire un cluster du parc et du fichier d'état.
    RemoveCluster {
        /// Nom local du cluster.
        name: String,
    },
    /// Fait du cluster nommé le cluster courant.
    SelectCluster {
        /// Nom local du cluster.
        name: String,
    },
    /// Recollecte l'état de tous les clusters connus.
    RefreshClusters {
        /// Identifiant de la requête.
        id: RequestId,
    },

    // -------------------------------------------------------------- lecture
    /// Synthèse de l'état d'un cluster (nœuds, pods, capacités).
    LoadOverview {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
    },
    /// Catalogue des types de ressources listables du cluster.
    LoadKinds {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
    },
    /// Noms des namespaces du cluster.
    LoadNamespaces {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
    },
    /// Liste paginée des objets d'un type donné.
    ListResources {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Type de ressource, tel que saisi (`po`, `pods`, `deployments.apps`...).
        kind: String,
        /// Namespace, sélecteurs, pagination.
        opts: ListOptions,
    },
    /// Lit d'un coup tous les objets nécessaires à l'écran « Topologie ».
    ///
    /// Pods, contrôleurs, services, ingress, volumes et nœuds sont listés en
    /// parallèle et paginés jusqu'au bout. Un type absent du cluster ou
    /// interdit par les droits produit un avertissement, pas un échec.
    LoadGraph {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Namespace ciblé ; `None` = tous les namespaces.
        namespace: Option<String>,
    },
    /// Manifeste YAML d'un objet précis.
    GetYaml {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Objet visé.
        reference: ResourceRef,
    },

    // -------------------------------------------------------------- écriture
    /// Applique un manifeste multi-documents (application côté serveur).
    ApplyYaml {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Manifeste complet.
        yaml: String,
        /// Simulation : rien n'est persisté côté serveur.
        dry_run: bool,
        /// Reprend de force les champs détenus par un autre gestionnaire.
        force: bool,
        /// Namespace appliqué aux documents qui n'en précisent pas.
        namespace: Option<String>,
    },
    /// Calcule la différence entre le cluster et un manifeste.
    DiffYaml {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Manifeste complet.
        yaml: String,
        /// Namespace appliqué aux documents qui n'en précisent pas.
        namespace: Option<String>,
    },
    /// Supprime toutes les ressources décrites par un manifeste.
    DeleteYaml {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Manifeste complet.
        yaml: String,
        /// Namespace appliqué aux documents qui n'en précisent pas.
        namespace: Option<String>,
    },
    /// Remplace intégralement un objet par le manifeste fourni.
    ReplaceYaml {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Objet visé.
        reference: ResourceRef,
        /// Manifeste d'un seul document.
        yaml: String,
    },
    /// Supprime un objet.
    DeleteResource {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Objet visé.
        reference: ResourceRef,
        /// `Foreground`, `Background` ou `Orphan`.
        propagation: Option<String>,
    },
    /// Change le nombre de répliques d'une charge de travail.
    Scale {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Objet visé.
        reference: ResourceRef,
        /// Nombre de répliques souhaité.
        replicas: i32,
    },
    /// Redémarrage progressif d'une charge de travail.
    Restart {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Objet visé.
        reference: ResourceRef,
    },
    /// Change l'image d'un conteneur.
    SetImage {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Objet visé.
        reference: ResourceRef,
        /// Conteneur ciblé ; `None` = le conteneur unique.
        container: Option<String>,
        /// Nouvelle référence d'image.
        image: String,
    },
    /// Retour à la révision précédente.
    Rollback {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Objet visé.
        reference: ResourceRef,
    },
    /// Rend un nœud non ordonnançable, ou de nouveau ordonnançable.
    Cordon {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Nom du nœud.
        node: String,
        /// `true` = non ordonnançable.
        on: bool,
    },
    /// Vide un nœud : cordon puis éviction des pods éligibles.
    Drain {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Nom du nœud.
        node: String,
    },

    // ------------------------------------------------------- détail d'un objet
    /// Évènements récents du cluster, éventuellement filtrés.
    LoadEvents {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Namespace ; `None` = tous.
        namespace: Option<String>,
        /// Restreint aux évènements d'un objet précis.
        involved: Option<ResourceRef>,
    },
    /// Conteneurs d'un pod, conteneurs d'initialisation compris.
    LoadContainers {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Pod visé.
        pod: ResourceRef,
    },

    // ---------------------------------------------------------------- flux
    /// Ouvre un flux de journaux ; chaque ligne devient un `Event::LogLine`.
    StartLogs {
        /// Identifiant de la requête, réutilisé par `StopLogs`.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Pod visé.
        pod: ResourceRef,
        /// Conteneur, suivi continu, nombre de lignes, horodatage.
        opts: LogOptions,
    },
    /// Interrompt le flux de journaux ouvert sous cet identifiant.
    StopLogs {
        /// Identifiant du flux à interrompre.
        id: RequestId,
    },
    /// Ouvre une session interactive dans un conteneur.
    StartExec {
        /// Identifiant de la requête, réutilisé par les autres commandes d'exec.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Pod visé.
        pod: ResourceRef,
        /// Conteneur ; `None` = le conteneur unique.
        container: Option<String>,
        /// Commande à lancer ; vide = détection d'un interpréteur.
        command: Vec<String>,
    },
    /// Pousse des octets sur l'entrée standard de la session.
    ExecInput {
        /// Identifiant de la session.
        id: RequestId,
        /// Octets saisis par l'utilisateur.
        data: Vec<u8>,
    },
    /// Notifie la session d'un changement de taille du terminal.
    ExecResize {
        /// Identifiant de la session.
        id: RequestId,
        /// Nombre de colonnes.
        cols: u16,
        /// Nombre de lignes.
        rows: u16,
    },
    /// Ferme la session interactive.
    StopExec {
        /// Identifiant de la session.
        id: RequestId,
    },

    // ------------------------------------------------------------- métriques
    /// Mesures de consommation des nœuds et des pods.
    LoadMetrics {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Namespace des pods mesurés ; `None` = tous.
        namespace: Option<String>,
    },

    // ------------------------------------------------------------------- hub
    /// Recherche d'images dans un registre.
    HubSearchImages {
        /// Identifiant de la requête.
        id: RequestId,
        /// Texte recherché.
        query: String,
        /// Registre interrogé.
        registry: RegistryKind,
    },
    /// Liste les tags d'une image.
    HubListTags {
        /// Identifiant de la requête.
        id: RequestId,
        /// Référence d'image (`nginx`, `ghcr.io/org/app:1.2`...).
        image: String,
    },
    /// Inspecte le manifeste et la configuration OCI d'une image.
    HubInspect {
        /// Identifiant de la requête.
        id: RequestId,
        /// Référence d'image complète.
        image: String,
    },
    /// Recherche de charts Helm.
    HubSearchCharts {
        /// Identifiant de la requête.
        id: RequestId,
        /// Texte recherché.
        query: String,
    },
    /// Catalogue d'applications prêtes à déployer, embarqué dans le binaire.
    HubCatalog {
        /// Identifiant de la requête.
        id: RequestId,
    },
    /// Génère les manifestes d'un déploiement sans rien envoyer au cluster.
    HubRender {
        /// Identifiant de la requête.
        id: RequestId,
        /// Description du déploiement.
        request: Box<DeployRequest>,
    },
    /// Génère puis applique les manifestes d'un déploiement.
    HubDeploy {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Description du déploiement.
        request: Box<DeployRequest>,
        /// Simulation : rien n'est persisté côté serveur.
        dry_run: bool,
    },

    // -------------------------------------------------------------- updater
    /// Surveillants et détections enregistrés.
    UpdatesList {
        /// Identifiant de la requête.
        id: RequestId,
    },
    /// Crée ou met à jour un surveillant.
    UpdatesUpsertWatcher {
        /// Identifiant de la requête.
        id: RequestId,
        /// Surveillant complet.
        watcher: Box<WatcherSpec>,
    },
    /// Supprime un surveillant.
    UpdatesRemoveWatcher {
        /// Identifiant de la requête.
        id: RequestId,
        /// Identifiant du surveillant.
        watcher_id: String,
    },
    /// Lance une campagne de vérification de tous les surveillants actifs.
    UpdatesCheck {
        /// Identifiant de la requête.
        id: RequestId,
    },
    /// Applique une mise à jour détectée.
    UpdatesApply {
        /// Identifiant de la requête.
        id: RequestId,
        /// Identifiant de la détection.
        finding_id: String,
    },
    /// Historique des déploiements pilotés par l'updater.
    UpdatesHistory {
        /// Identifiant de la requête.
        id: RequestId,
    },
    /// Inventorie les images utilisées par les charges de travail d'un cluster.
    UpdatesScan {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Namespace ; `None` = tous.
        namespace: Option<String>,
    },
    /// Propose des surveillants à partir de l'inventaire des images.
    UpdatesSuggest {
        /// Identifiant de la requête.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Namespace ; `None` = tous.
        namespace: Option<String>,
    },
    /// Réglages de l'updater, secrets masqués.
    UpdatesSettings {
        /// Identifiant de la requête.
        id: RequestId,
    },
    /// Enregistre les réglages de l'updater.
    ///
    /// Un secret égal au masque d'affichage signifie « inchangé ».
    UpdatesSaveSettings {
        /// Identifiant de la requête.
        id: RequestId,
        /// Réglages complets.
        settings: Box<UpdaterSettings>,
    },

    /// Arrête le worker et abandonne toutes les tâches en cours.
    Quit,
}

impl Command {
    /// Identifiant de la requête, lorsqu'elle en porte un.
    pub fn id(&self) -> Option<RequestId> {
        match self {
            Command::Connect { id, .. }
            | Command::ImportKubeconfig { id, .. }
            | Command::ListContexts { id, .. }
            | Command::RefreshClusters { id }
            | Command::LoadOverview { id, .. }
            | Command::LoadKinds { id, .. }
            | Command::LoadNamespaces { id, .. }
            | Command::ListResources { id, .. }
            | Command::LoadGraph { id, .. }
            | Command::GetYaml { id, .. }
            | Command::ApplyYaml { id, .. }
            | Command::DiffYaml { id, .. }
            | Command::DeleteYaml { id, .. }
            | Command::ReplaceYaml { id, .. }
            | Command::DeleteResource { id, .. }
            | Command::Scale { id, .. }
            | Command::Restart { id, .. }
            | Command::SetImage { id, .. }
            | Command::Rollback { id, .. }
            | Command::Cordon { id, .. }
            | Command::Drain { id, .. }
            | Command::LoadEvents { id, .. }
            | Command::LoadContainers { id, .. }
            | Command::StartLogs { id, .. }
            | Command::StopLogs { id }
            | Command::StartExec { id, .. }
            | Command::ExecInput { id, .. }
            | Command::ExecResize { id, .. }
            | Command::StopExec { id }
            | Command::LoadMetrics { id, .. }
            | Command::HubSearchImages { id, .. }
            | Command::HubListTags { id, .. }
            | Command::HubInspect { id, .. }
            | Command::HubSearchCharts { id, .. }
            | Command::HubCatalog { id }
            | Command::HubRender { id, .. }
            | Command::HubDeploy { id, .. }
            | Command::UpdatesList { id }
            | Command::UpdatesUpsertWatcher { id, .. }
            | Command::UpdatesRemoveWatcher { id, .. }
            | Command::UpdatesCheck { id }
            | Command::UpdatesApply { id, .. }
            | Command::UpdatesHistory { id }
            | Command::UpdatesScan { id, .. }
            | Command::UpdatesSuggest { id, .. }
            | Command::UpdatesSettings { id }
            | Command::UpdatesSaveSettings { id, .. } => Some(*id),
            Command::RemoveCluster { .. } | Command::SelectCluster { .. } | Command::Quit => None,
        }
    }

    /// Libellé français affiché par l'interface tant que la requête est en vol.
    ///
    /// `None` pour les commandes qui n'occupent pas l'utilisateur : ordres immédiats,
    /// interruptions de flux, saisie clavier d'une session interactive.
    pub fn busy_label(&self) -> Option<String> {
        let label = match self {
            Command::Connect { name, .. } => format!("Connexion à « {name} »"),
            Command::ImportKubeconfig { .. } => "Import du kubeconfig".to_string(),
            Command::ListContexts { .. } => "Lecture des contextes".to_string(),
            Command::RefreshClusters { .. } => "Actualisation des clusters".to_string(),
            Command::LoadOverview { cluster, .. } => format!("Synthèse de « {cluster} »"),
            Command::LoadKinds { .. } => "Découverte des types de ressources".to_string(),
            Command::LoadNamespaces { .. } => "Lecture des namespaces".to_string(),
            Command::ListResources { kind, .. } => format!("Listing des {kind}"),
            Command::LoadGraph { .. } => "Lecture de la topologie".to_string(),
            Command::GetYaml { reference, .. } => format!("Lecture de {}", reference.display()),
            Command::ApplyYaml { dry_run, .. } => {
                if *dry_run {
                    "Simulation de l'application".to_string()
                } else {
                    "Application du manifeste".to_string()
                }
            }
            Command::DiffYaml { .. } => "Calcul des différences".to_string(),
            Command::DeleteYaml { .. } => "Suppression du manifeste".to_string(),
            Command::ReplaceYaml { reference, .. } => {
                format!("Remplacement de {}", reference.display())
            }
            Command::DeleteResource { reference, .. } => {
                format!("Suppression de {}", reference.display())
            }
            Command::Scale {
                reference,
                replicas,
                ..
            } => format!(
                "Passage de {} à {replicas} réplique(s)",
                reference.display()
            ),
            Command::Restart { reference, .. } => {
                format!("Redémarrage de {}", reference.display())
            }
            Command::SetImage {
                reference, image, ..
            } => format!("Image de {} → {image}", reference.display()),
            Command::Rollback { reference, .. } => {
                format!("Retour arrière de {}", reference.display())
            }
            Command::Cordon { node, on, .. } => {
                if *on {
                    format!("Mise hors service du nœud « {node} »")
                } else {
                    format!("Remise en service du nœud « {node} »")
                }
            }
            Command::Drain { node, .. } => format!("Vidage du nœud « {node} »"),
            Command::LoadEvents { .. } => "Lecture des évènements".to_string(),
            Command::LoadContainers { .. } => "Lecture des conteneurs".to_string(),
            Command::StartLogs { pod, .. } => format!("Journaux de {}", pod.display()),
            Command::StartExec { pod, .. } => format!("Session sur {}", pod.display()),
            Command::LoadMetrics { .. } => "Lecture des métriques".to_string(),
            Command::HubSearchImages { query, .. } => format!("Recherche d'images « {query} »"),
            Command::HubListTags { image, .. } => format!("Tags de « {image} »"),
            Command::HubInspect { image, .. } => format!("Inspection de « {image} »"),
            Command::HubSearchCharts { query, .. } => format!("Recherche de charts « {query} »"),
            Command::HubCatalog { .. } => "Chargement du catalogue".to_string(),
            Command::HubRender { .. } => "Génération des manifestes".to_string(),
            Command::HubDeploy { request, .. } => format!("Déploiement de « {} »", request.name),
            Command::UpdatesList { .. } => "Lecture des surveillants".to_string(),
            Command::UpdatesUpsertWatcher { watcher, .. } => {
                format!("Enregistrement de « {} »", watcher.name)
            }
            Command::UpdatesRemoveWatcher { .. } => "Suppression du surveillant".to_string(),
            Command::UpdatesCheck { .. } => "Vérification des mises à jour".to_string(),
            Command::UpdatesApply { .. } => "Application de la mise à jour".to_string(),
            Command::UpdatesHistory { .. } => "Lecture de l'historique".to_string(),
            Command::UpdatesScan { cluster, .. } => {
                format!("Inventaire des images de « {cluster} »")
            }
            Command::UpdatesSuggest { .. } => "Suggestion de surveillants".to_string(),
            Command::UpdatesSettings { .. } => "Lecture des réglages".to_string(),
            Command::UpdatesSaveSettings { .. } => "Enregistrement des réglages".to_string(),

            // Ordres immédiats et trafic de session : aucun indicateur d'attente.
            Command::RemoveCluster { .. }
            | Command::SelectCluster { .. }
            | Command::StopLogs { .. }
            | Command::ExecInput { .. }
            | Command::ExecResize { .. }
            | Command::StopExec { .. }
            | Command::Quit => return None,
        };
        Some(label)
    }

    /// Vrai si la commande doit être traitée en ligne par la boucle du worker.
    ///
    /// Ces commandes sont immédiates et ne font aucune entrée/sortie réseau : les
    /// confier à une tâche les placerait derrière les commandes lentes déjà en vol,
    /// alors qu'elles servent précisément à les interrompre ou à changer de cible.
    pub fn is_immediate(&self) -> bool {
        matches!(
            self,
            Command::SelectCluster { .. }
                | Command::RemoveCluster { .. }
                | Command::StopLogs { .. }
                | Command::StopExec { .. }
                | Command::ExecInput { .. }
                | Command::ExecResize { .. }
                | Command::Quit
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiants_et_ordres_immediats() {
        let c = Command::LoadOverview {
            id: 7,
            cluster: "prod".into(),
        };
        assert_eq!(c.id(), Some(7));
        assert!(!c.is_immediate());

        let q = Command::Quit;
        assert_eq!(q.id(), None);
        assert!(q.is_immediate());

        let s = Command::SelectCluster {
            name: "staging".into(),
        };
        assert_eq!(s.id(), None);
        assert!(s.is_immediate());

        // `StopLogs` porte l'identifiant du flux, mais reste un ordre immédiat.
        let stop = Command::StopLogs { id: 42 };
        assert_eq!(stop.id(), Some(42));
        assert!(stop.is_immediate());
    }

    #[test]
    fn libelles_en_francais() {
        let c = Command::Connect {
            id: 1,
            spec: ConnectionSpec::default_kubeconfig(),
            name: "prod".into(),
            persist: true,
        };
        assert_eq!(c.busy_label().as_deref(), Some("Connexion à « prod »"));

        // Le trafic clavier d'une session n'occupe pas l'utilisateur.
        assert!(Command::ExecInput {
            id: 1,
            data: vec![]
        }
        .busy_label()
        .is_none());
        assert!(Command::Quit.busy_label().is_none());
    }

    #[test]
    fn serialisation_aller_retour() {
        let cmd = Command::ListResources {
            id: 12,
            cluster: "prod".into(),
            kind: "pods".into(),
            opts: ListOptions {
                namespace: Some("kube-system".into()),
                label_selector: Some("app=web".into()),
                limit: Some(250),
                ..Default::default()
            },
        };
        let json = serde_json::to_string(&cmd).expect("sérialisation");
        let back: Command = serde_json::from_str(&json).expect("désérialisation");
        match back {
            Command::ListResources { id, kind, opts, .. } => {
                assert_eq!(id, 12);
                assert_eq!(kind, "pods");
                assert_eq!(opts.limit, Some(250));
            }
            other => panic!("variante inattendue : {other:?}"),
        }
    }

    #[test]
    fn identifiant_reserve_distinct_des_requetes() {
        // `AppState::next_id()` commence à 1 : aucune requête ne peut usurper
        // l'identifiant des évènements spontanés du worker.
        assert_eq!(NO_REQUEST, 0);
    }
}
