//! Évènements renvoyés par le worker asynchrone à l'interface.
//!
//! Un [`Event`] est toujours la réponse à une [`crate::backend::Command`] : il en
//! reprend le [`RequestId`], ce qui permet à l'interface d'écarter les réponses
//! devenues obsolètes. Seuls les rafraîchissements spontanés du worker (chargement
//! initial des clusters, suite d'une sélection ou d'une suppression) portent
//! [`NO_REQUEST`].
//!
//! Toute erreur d'exécution produit un [`Event::Failed`] porteur d'un message
//! français lisible : le worker ne se tait jamais et ne panique jamais.

use serde::{Deserialize, Serialize};

use kubewatch_core::apply::ApplyOutcome;
use kubewatch_core::model::{
    ClusterInfo, ClusterOverview, ContainerInfo, ContextInfo, EventSummary, MetricsSample,
    ObjectListPage, ObjectSummary, ResourceKind, ResourceRef,
};
use kubewatch_hub::model::{CatalogApp, ChartSummary, ImageDetails, ImageSummary, TagInfo};
use kubewatch_updater::model::{RolloutResult, UpdateFinding, WatcherSpec};
use kubewatch_updater::scan::WorkloadImage;
use kubewatch_updater::store::UpdaterSettings;

use super::command::RequestId;

/// Réponse du worker à une commande.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum Event {
    // ------------------------------------------------------------- clusters
    /// État de tous les clusters connus, connectés ou simplement enregistrés.
    ///
    /// Avec `id == NO_REQUEST`, il s'agit d'un rafraîchissement spontané : la vue
    /// doit remplacer sa liste sans chercher de requête en attente.
    Clusters {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Clusters connus.
        clusters: Vec<ClusterInfo>,
    },
    /// Contextes déclarés par un kubeconfig.
    Contexts {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Contextes lus.
        contexts: Vec<ContextInfo>,
    },

    // -------------------------------------------------------------- lecture
    /// Synthèse de l'état d'un cluster.
    Overview {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Synthèse collectée.
        overview: Box<ClusterOverview>,
    },
    /// Types de ressources listables d'un cluster.
    Kinds {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Catalogue trié.
        kinds: Vec<ResourceKind>,
    },
    /// Namespaces d'un cluster.
    Namespaces {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Noms des namespaces.
        namespaces: Vec<String>,
    },
    /// Page d'objets d'un type donné.
    Resources {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Type demandé, tel que résolu par le catalogue.
        kind: String,
        /// Objets et jeton de continuation.
        page: Box<ObjectListPage>,
    },
    /// Instantané des objets de l'écran « Topologie ».
    Graph {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Nom local du cluster.
        cluster: String,
        /// Objets lus, tous types confondus.
        objects: Vec<ObjectSummary>,
        /// Types qui n'ont pas pu être lus, avec la cause.
        warnings: Vec<String>,
    },
    /// Manifeste YAML d'un objet.
    Yaml {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Objet lu.
        reference: ResourceRef,
        /// Manifeste, `managedFields` retirés.
        yaml: String,
    },

    // ------------------------------------------------------------- écriture
    /// Bilan d'une application, d'une suppression ou d'un déploiement par manifeste.
    ApplyOutcome {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Un élément par document traité.
        outcome: Box<ApplyOutcome>,
    },
    /// Différences entre le cluster et un manifeste, un couple par document.
    Diff {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Ressource visée et différence au format texte.
        items: Vec<(ResourceRef, String)>,
    },

    // ------------------------------------------------------- détail d'un objet
    /// Évènements Kubernetes récents.
    Events {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Évènements, du plus récent au plus ancien.
        events: Vec<EventSummary>,
    },
    /// Conteneurs d'un pod.
    Containers {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Conteneurs, conteneurs d'initialisation compris.
        containers: Vec<ContainerInfo>,
    },

    // ---------------------------------------------------------------- flux
    /// Fragment de journal.
    ///
    /// En régime normal, une ligne par évènement. En rafale (plus de
    /// `LOG_BURST_LINES` lignes en une seconde), le worker regroupe les lignes :
    /// `line` contient alors plusieurs lignes séparées par `\n`. Le lecteur doit
    /// donc découper sur `\n` plutôt que supposer une ligne unique.
    LogLine {
        /// Identifiant du flux.
        id: RequestId,
        /// Une ou plusieurs lignes séparées par `\n`.
        line: String,
    },
    /// Le flux de journaux s'est terminé (conteneur arrêté, flux interrompu).
    LogEnded {
        /// Identifiant du flux.
        id: RequestId,
    },
    /// Octets produits par une session interactive (stdout et stderr mêlés).
    ExecOutput {
        /// Identifiant de la session.
        id: RequestId,
        /// Octets bruts, à interpréter par l'émulateur de terminal.
        data: Vec<u8>,
    },
    /// La session interactive est close.
    ExecEnded {
        /// Identifiant de la session.
        id: RequestId,
        /// Raison de la fermeture, le cas échéant.
        message: Option<String>,
    },

    // ------------------------------------------------------------- métriques
    /// Mesures de consommation.
    Metrics {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Mesures par nœud.
        nodes: Vec<MetricsSample>,
        /// Mesures par pod.
        pods: Vec<MetricsSample>,
    },

    // ------------------------------------------------------------------- hub
    /// Résultats d'une recherche d'images.
    HubImages {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Images trouvées.
        images: Vec<ImageSummary>,
    },
    /// Tags d'une image, du plus récent au plus ancien.
    HubTags {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Tags trouvés.
        tags: Vec<TagInfo>,
    },
    /// Détail d'une image (digest, ports, variables, labels).
    HubDetails {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Détail extrait du manifeste OCI.
        details: Box<ImageDetails>,
    },
    /// Résultats d'une recherche de charts Helm.
    HubCharts {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Charts trouvés.
        charts: Vec<ChartSummary>,
    },
    /// Catalogue d'applications prêtes à déployer.
    HubCatalog {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Applications du catalogue intégré.
        apps: Vec<CatalogApp>,
    },
    /// Manifestes générés pour un déploiement.
    HubRendered {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Manifeste multi-documents.
        yaml: String,
    },

    // -------------------------------------------------------------- updater
    /// Surveillants et détections enregistrés.
    Watchers {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Surveillants configurés.
        watchers: Vec<WatcherSpec>,
        /// Détections en attente.
        findings: Vec<UpdateFinding>,
    },
    /// Détections issues d'une campagne de vérification.
    Findings {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Mises à jour disponibles.
        findings: Vec<UpdateFinding>,
    },
    /// Résultat d'un déploiement piloté par l'updater.
    RolloutDone {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Détail du déploiement.
        result: Box<RolloutResult>,
    },
    /// Historique des déploiements pilotés par l'updater.
    History {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Déploiements passés, du plus récent au plus ancien.
        history: Vec<RolloutResult>,
    },
    /// Inventaire des images utilisées par les charges de travail.
    WorkloadImages {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Une entrée par couple (charge de travail, conteneur).
        images: Vec<WorkloadImage>,
    },
    /// Surveillants proposés à partir de l'inventaire.
    Suggestions {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Surveillants prêts à être enregistrés.
        watchers: Vec<WatcherSpec>,
    },
    /// Réglages de l'updater, secrets masqués.
    Settings {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Réglages courants.
        settings: Box<UpdaterSettings>,
    },

    // ------------------------------------------------------------- généraux
    /// Succès d'une commande sans autre résultat à renvoyer.
    Ok {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Message de confirmation, en français.
        message: String,
    },
    /// Échec d'une commande.
    ///
    /// Le message est destiné à l'utilisateur : il doit rester lisible et en français.
    Failed {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Cause de l'échec.
        message: String,
    },
    /// La commande vient d'être prise en charge ; l'interface peut afficher une attente.
    Busy {
        /// Identifiant de la requête d'origine.
        id: RequestId,
        /// Libellé français de l'opération en cours.
        what: String,
    },
}

impl Event {
    /// Identifiant de la requête à laquelle cet évènement répond.
    pub fn id(&self) -> RequestId {
        match self {
            Event::Clusters { id, .. }
            | Event::Contexts { id, .. }
            | Event::Overview { id, .. }
            | Event::Kinds { id, .. }
            | Event::Namespaces { id, .. }
            | Event::Resources { id, .. }
            | Event::Graph { id, .. }
            | Event::Yaml { id, .. }
            | Event::ApplyOutcome { id, .. }
            | Event::Diff { id, .. }
            | Event::Events { id, .. }
            | Event::Containers { id, .. }
            | Event::LogLine { id, .. }
            | Event::LogEnded { id }
            | Event::ExecOutput { id, .. }
            | Event::ExecEnded { id, .. }
            | Event::Metrics { id, .. }
            | Event::HubImages { id, .. }
            | Event::HubTags { id, .. }
            | Event::HubDetails { id, .. }
            | Event::HubCharts { id, .. }
            | Event::HubCatalog { id, .. }
            | Event::HubRendered { id, .. }
            | Event::Watchers { id, .. }
            | Event::Findings { id, .. }
            | Event::RolloutDone { id, .. }
            | Event::History { id, .. }
            | Event::WorkloadImages { id, .. }
            | Event::Suggestions { id, .. }
            | Event::Settings { id, .. }
            | Event::Ok { id, .. }
            | Event::Failed { id, .. }
            | Event::Busy { id, .. } => *id,
        }
    }

    /// Vrai si l'évènement clôt la requête : l'interface peut retirer l'entrée
    /// correspondante de `AppState::pending`.
    ///
    /// `Busy` ouvre l'attente, `LogLine` et `ExecOutput` la prolongent (le flux
    /// reste ouvert) ; tout le reste la termine.
    pub fn is_terminal(&self) -> bool {
        !matches!(
            self,
            Event::Busy { .. } | Event::LogLine { .. } | Event::ExecOutput { .. }
        )
    }

    /// Message d'erreur, lorsque l'évènement en porte un.
    pub fn failure(&self) -> Option<&str> {
        match self {
            Event::Failed { message, .. } => Some(message.as_str()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::command::NO_REQUEST;

    #[test]
    fn identifiant_de_chaque_evenement() {
        assert_eq!(
            Event::Ok {
                id: 5,
                message: "fait".into()
            }
            .id(),
            5
        );
        assert_eq!(
            Event::Clusters {
                id: NO_REQUEST,
                clusters: Vec::new()
            }
            .id(),
            NO_REQUEST
        );
    }

    #[test]
    fn seuls_les_flux_et_busy_ne_cloturent_pas() {
        assert!(!Event::Busy {
            id: 1,
            what: "Connexion".into()
        }
        .is_terminal());
        assert!(!Event::LogLine {
            id: 1,
            line: "a".into()
        }
        .is_terminal());
        assert!(!Event::ExecOutput {
            id: 1,
            data: vec![b'x']
        }
        .is_terminal());
        assert!(Event::LogEnded { id: 1 }.is_terminal());
        assert!(Event::Failed {
            id: 1,
            message: "cluster injoignable".into()
        }
        .is_terminal());
    }

    #[test]
    fn message_d_echec_accessible() {
        let e = Event::Failed {
            id: 3,
            message: "cluster « prod » non connecté".into(),
        };
        assert_eq!(e.failure(), Some("cluster « prod » non connecté"));
        assert!(Event::LogEnded { id: 3 }.failure().is_none());
    }

    #[test]
    fn serialisation_aller_retour() {
        let ev = Event::Namespaces {
            id: 9,
            cluster: "prod".into(),
            namespaces: vec!["default".into(), "kube-system".into()],
        };
        let json = serde_json::to_string(&ev).expect("sérialisation");
        let back: Event = serde_json::from_str(&json).expect("désérialisation");
        match back {
            Event::Namespaces {
                id,
                cluster,
                namespaces,
            } => {
                assert_eq!(id, 9);
                assert_eq!(cluster, "prod");
                assert_eq!(namespaces.len(), 2);
            }
            other => panic!("variante inattendue : {other:?}"),
        }
    }

    #[test]
    fn regroupement_de_lignes_documente() {
        // Le contrat de `LogLine` autorise plusieurs lignes dans un seul évènement.
        let ev = Event::LogLine {
            id: 1,
            line: "premiere\nseconde".into(),
        };
        match &ev {
            Event::LogLine { line, .. } => assert_eq!(line.lines().count(), 2),
            other => panic!("variante inattendue : {other:?}"),
        }
    }
}
