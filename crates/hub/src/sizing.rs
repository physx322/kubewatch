//! Profils de taille et prévision de l'empreinte d'un déploiement.
//!
//! Ce module répond à une question que se pose tout utilisateur avant de
//! déployer : « combien ça va coûter à mon cluster, et est-ce que ça tient ? »
//!
//! Il ne parle à personne : il transforme une [`DeployRequest`] et une
//! [`ClusterOverview`] en une [`Assessment`] affichable. Tout est calculable
//! hors ligne, donc entièrement testable sans cluster ni fenêtre.
//!
//! ## Ce que la prévision mesure exactement
//!
//! L'empreinte additionnée est celle des **demandes** (`resources.requests`),
//! car c'est sur elles que l'ordonnanceur de Kubernetes réserve la place. Le
//! point de départ, lui, est la **consommation mesurée** du cluster
//! (`metrics.k8s.io`), seule donnée dont dispose la vue d'ensemble. Comparer
//! les deux donne un ordre de grandeur honnête, pas une simulation
//! d'ordonnanceur : un cluster peut refuser un pod qui « tient » ici parce que
//! ses réservations existantes, invisibles dans la consommation mesurée,
//! saturent déjà les nœuds. Les libellés de l'interface le disent.

use serde::{Deserialize, Serialize};

use kubewatch_core::metrics::{parse_cpu_millis, parse_memory_bytes};
use kubewatch_core::model::ClusterOverview;

use crate::deploy::DeployRequest;
use crate::model::CatalogApp;

/// Seuil au-delà duquel l'occupation prévue est jugée serrée.
const TIGHT_FRACTION: f64 = 0.85;

/// Un gibioctet, en octets.
const GIB: i64 = 1024 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Profils de taille
// ---------------------------------------------------------------------------

/// Gabarit de ressources proposé à l'utilisateur à la place des quatre
/// quantités Kubernetes brutes.
///
/// Les valeurs sont volontairement modestes : un profil trop généreux empêche
/// l'ordonnancement sur un petit cluster, alors qu'un profil trop juste ne
/// provoque au pire qu'un étranglement CPU, visible et corrigeable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SizeProfile {
    /// Outil d'appoint, page statique, tableau de bord consulté de loin en loin.
    Micro,
    /// Petite API, service métier peu sollicité.
    #[default]
    Small,
    /// Base de données modeste, service au trafic régulier.
    Medium,
    /// Base de données chargée, traitement gourmand.
    Large,
    /// Quantités saisies à la main dans le formulaire avancé.
    Custom,
}

/// Les quatre quantités Kubernetes d'un profil.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileResources {
    /// Demande CPU (`resources.requests.cpu`).
    pub cpu_request: &'static str,
    /// Limite CPU (`resources.limits.cpu`).
    pub cpu_limit: &'static str,
    /// Demande mémoire (`resources.requests.memory`).
    pub memory_request: &'static str,
    /// Limite mémoire (`resources.limits.memory`).
    pub memory_limit: &'static str,
}

impl SizeProfile {
    /// Les profils chiffrés, dans l'ordre croissant ; `Custom` en est exclu.
    pub const PRESETS: [SizeProfile; 4] = [
        SizeProfile::Micro,
        SizeProfile::Small,
        SizeProfile::Medium,
        SizeProfile::Large,
    ];

    /// Nom affiché.
    pub fn label(self) -> &'static str {
        match self {
            SizeProfile::Micro => "Micro",
            SizeProfile::Small => "Petit",
            SizeProfile::Medium => "Moyen",
            SizeProfile::Large => "Grand",
            SizeProfile::Custom => "Personnalisé",
        }
    }

    /// Phrase qui dit à quel usage le profil correspond.
    pub fn description(self) -> &'static str {
        match self {
            SizeProfile::Micro => "Page statique, outil d'appoint, démonstration.",
            SizeProfile::Small => "API légère, service métier peu sollicité.",
            SizeProfile::Medium => "Base de données modeste, service au trafic régulier.",
            SizeProfile::Large => "Base de données chargée, traitement gourmand.",
            SizeProfile::Custom => "Quantités saisies à la main.",
        }
    }

    /// Quantités du profil, ou `None` pour `Custom`.
    pub fn resources(self) -> Option<ProfileResources> {
        let r = match self {
            SizeProfile::Micro => ProfileResources {
                cpu_request: "10m",
                cpu_limit: "200m",
                memory_request: "32Mi",
                memory_limit: "128Mi",
            },
            SizeProfile::Small => ProfileResources {
                cpu_request: "50m",
                cpu_limit: "500m",
                memory_request: "128Mi",
                memory_limit: "512Mi",
            },
            SizeProfile::Medium => ProfileResources {
                cpu_request: "250m",
                cpu_limit: "1",
                memory_request: "512Mi",
                memory_limit: "2Gi",
            },
            SizeProfile::Large => ProfileResources {
                cpu_request: "1",
                cpu_limit: "2",
                memory_request: "2Gi",
                memory_limit: "4Gi",
            },
            SizeProfile::Custom => return None,
        };
        Some(r)
    }

    /// Écrit les quantités du profil dans la requête ; `Custom` n'y touche pas.
    pub fn apply_to(self, req: &mut DeployRequest) {
        let Some(r) = self.resources() else {
            return;
        };
        req.cpu_request = Some(r.cpu_request.to_string());
        req.cpu_limit = Some(r.cpu_limit.to_string());
        req.memory_request = Some(r.memory_request.to_string());
        req.memory_limit = Some(r.memory_limit.to_string());
    }

    /// Reconnaît le profil déjà inscrit dans une requête.
    ///
    /// La comparaison porte sur les valeurs analysées, pas sur le texte :
    /// `1`, `1000m` et `1.0` décrivent le même cœur.
    pub fn detect(req: &DeployRequest) -> SizeProfile {
        SizeProfile::PRESETS
            .into_iter()
            .find(|p| {
                let r = p
                    .resources()
                    .expect("un préréglage porte toujours ses quantités");
                same_cpu(req.cpu_request.as_deref(), r.cpu_request)
                    && same_cpu(req.cpu_limit.as_deref(), r.cpu_limit)
                    && same_memory(req.memory_request.as_deref(), r.memory_request)
                    && same_memory(req.memory_limit.as_deref(), r.memory_limit)
            })
            .unwrap_or(SizeProfile::Custom)
    }
}

/// Profil conseillé pour une application du catalogue.
///
/// Le catalogue ne porte pas de dimensionnement : la catégorie est le seul
/// indice disponible, et elle suffit à distinguer un serveur web d'une base de
/// données. L'utilisateur reste libre de changer, c'est un point de départ.
pub fn recommended_profile(app: &CatalogApp) -> SizeProfile {
    match app.category.as_str() {
        "Base de données" => SizeProfile::Medium,
        "Observabilité" | "Stockage" | "Automatisation" | "Gestion de code" => SizeProfile::Medium,
        "Cache" | "File d'attente" => SizeProfile::Small,
        _ if app.needs_pvc => SizeProfile::Medium,
        _ => SizeProfile::Small,
    }
}

/// Vrai si deux quantités CPU décrivent la même valeur.
fn same_cpu(left: Option<&str>, right: &str) -> bool {
    match (left.map(str::trim).filter(|v| !v.is_empty()), right) {
        (Some(l), r) => match (parse_cpu_millis(l), parse_cpu_millis(r)) {
            (Some(a), Some(b)) => (a - b).abs() < 0.001,
            _ => false,
        },
        (None, _) => false,
    }
}

/// Vrai si deux quantités mémoire décrivent la même valeur.
fn same_memory(left: Option<&str>, right: &str) -> bool {
    match left.map(str::trim).filter(|v| !v.is_empty()) {
        Some(l) => parse_memory_bytes(l) == parse_memory_bytes(right),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Empreinte
// ---------------------------------------------------------------------------

/// Ce qu'un déploiement va réserver sur le cluster.
///
/// Les champs CPU et mémoire sont déjà multipliés par le nombre de répliques.
/// Le stockage ne l'est pas : le générateur de manifestes crée un seul
/// `PersistentVolumeClaim`, monté par tous les pods du `Deployment`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Footprint {
    /// Nombre de pods créés.
    pub replicas: i32,
    /// Demande CPU cumulée, en millicores.
    pub cpu_request_millis: Option<f64>,
    /// Limite CPU cumulée, en millicores.
    pub cpu_limit_millis: Option<f64>,
    /// Demande mémoire cumulée, en octets.
    pub memory_request_bytes: Option<i64>,
    /// Limite mémoire cumulée, en octets.
    pub memory_limit_bytes: Option<i64>,
    /// Stockage persistant demandé, en octets.
    pub storage_bytes: Option<i64>,
}

/// Calcule l'empreinte d'une requête de déploiement.
///
/// Une quantité absente ou illisible reste `None` : mieux vaut afficher un
/// tiret qu'un zéro, qui laisserait croire que rien n'est consommé.
pub fn footprint(req: &DeployRequest) -> Footprint {
    let replicas = req.replicas.max(0);
    let factor = f64::from(replicas);

    let cpu = |q: &Option<String>| -> Option<f64> {
        parse_cpu_millis(q.as_deref()?.trim()).map(|v| v * factor)
    };
    let memory = |q: &Option<String>| -> Option<i64> {
        let bytes = parse_memory_bytes(q.as_deref()?.trim())?;
        bytes.checked_mul(i64::from(replicas))
    };

    Footprint {
        replicas,
        cpu_request_millis: cpu(&req.cpu_request),
        cpu_limit_millis: cpu(&req.cpu_limit),
        memory_request_bytes: memory(&req.memory_request),
        memory_limit_bytes: memory(&req.memory_limit),
        storage_bytes: req
            .pvc
            .as_ref()
            .and_then(|p| parse_memory_bytes(p.size.trim())),
    }
}

// ---------------------------------------------------------------------------
// Confrontation à la capacité du cluster
// ---------------------------------------------------------------------------

/// Un axe de capacité (CPU ou mémoire) avant et après le déploiement.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Axis {
    /// Consommation mesurée avant le déploiement, dans l'unité de l'axe.
    pub used: f64,
    /// Ce que le déploiement demande en plus.
    pub added: f64,
    /// Capacité totale du cluster.
    pub capacity: f64,
}

impl Axis {
    /// Part occupée avant le déploiement, bornée à 1.
    pub fn before_fraction(&self) -> f32 {
        fraction(self.used, self.capacity)
    }

    /// Part occupée après le déploiement, bornée à 1.
    pub fn after_fraction(&self) -> f32 {
        fraction(self.used + self.added, self.capacity)
    }

    /// Part occupée après le déploiement, sans borne : au-delà de 1, ça déborde.
    pub fn after_ratio(&self) -> f64 {
        if self.capacity <= 0.0 {
            return 0.0;
        }
        (self.used + self.added) / self.capacity
    }

    /// Ce qui reste libre après le déploiement ; négatif en cas de dépassement.
    pub fn remaining(&self) -> f64 {
        self.capacity - self.used - self.added
    }
}

/// Part d'un total, bornée à `[0, 1]` pour un usage direct en barre de progression.
fn fraction(value: f64, total: f64) -> f32 {
    if total <= 0.0 || !value.is_finite() {
        return 0.0;
    }
    (value / total).clamp(0.0, 1.0) as f32
}

/// Verdict global de la prévision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fit {
    /// La place restante est confortable.
    Fits,
    /// Le déploiement passe, mais le cluster finit au-dessus de 85 % d'occupation.
    Tight,
    /// La demande dépasse la capacité d'au moins un axe.
    Exceeds,
    /// Capacité ou consommation inconnues : rien à comparer.
    #[default]
    Unknown,
}

impl Fit {
    /// Phrase affichée en tête de l'aperçu.
    pub fn headline(self) -> &'static str {
        match self {
            Fit::Fits => "Ce déploiement tient sans difficulté.",
            Fit::Tight => "Ce déploiement passe, mais le cluster sera chargé.",
            Fit::Exceeds => "Ce déploiement dépasse la capacité du cluster.",
            Fit::Unknown => "Consommation du cluster inconnue : la prévision est incomplète.",
        }
    }
}

/// Gravité d'une remarque de la prévision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NoteLevel {
    /// Information utile, aucune action requise.
    Info,
    /// Choix qui fonctionnera mais se paiera plus tard.
    Warning,
    /// Le déploiement sera refusé ou restera bloqué en l'état.
    Danger,
}

/// Une remarque adressée à l'utilisateur avant qu'il ne déploie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// Gravité, qui décide de la couleur.
    pub level: NoteLevel,
    /// Texte complet, en français, qui dit la conséquence et non la règle.
    pub text: String,
}

impl Note {
    /// Construit une remarque.
    fn new(level: NoteLevel, text: impl Into<String>) -> Self {
        Note {
            level,
            text: text.into(),
        }
    }
}

/// Prévision complète, prête à afficher.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Assessment {
    /// Empreinte calculée.
    pub footprint: Footprint,
    /// Axe CPU, en millicores ; absent si le cluster n'est pas mesuré.
    pub cpu: Option<Axis>,
    /// Axe mémoire, en octets convertis en `f64` ; absent si le cluster n'est pas mesuré.
    pub memory: Option<Axis>,
    /// Verdict global.
    pub fit: Fit,
    /// Remarques, les plus graves en tête.
    pub notes: Vec<Note>,
}

/// Confronte une requête de déploiement à l'état connu du cluster.
///
/// `overview` vaut `None` tant que la synthèse n'a pas été chargée : la
/// prévision se limite alors aux remarques qui ne dépendent pas du cluster.
pub fn assess(req: &DeployRequest, overview: Option<&ClusterOverview>) -> Assessment {
    let footprint = footprint(req);
    let mut notes = Vec::new();

    let measurable = overview.filter(|o| o.metrics_available);
    let cpu = measurable.and_then(|o| {
        (o.cpu_capacity_millis > 0.0).then(|| Axis {
            used: o.cpu_used_millis,
            added: footprint.cpu_request_millis.unwrap_or(0.0),
            capacity: o.cpu_capacity_millis,
        })
    });
    let memory = measurable.and_then(|o| {
        (o.memory_capacity_bytes > 0).then(|| Axis {
            used: o.memory_used_bytes as f64,
            added: footprint.memory_request_bytes.unwrap_or(0) as f64,
            capacity: o.memory_capacity_bytes as f64,
        })
    });

    let fit = match (cpu, memory) {
        (None, None) => Fit::Unknown,
        (a, b) => {
            let ratios: Vec<f64> = [a, b]
                .into_iter()
                .flatten()
                .map(|x| x.after_ratio())
                .collect();
            let worst = ratios.into_iter().fold(0.0_f64, f64::max);
            if worst > 1.0 {
                Fit::Exceeds
            } else if worst > TIGHT_FRACTION {
                Fit::Tight
            } else {
                Fit::Fits
            }
        }
    };

    collect_notes(req, &footprint, cpu, memory, &mut notes);
    // Le plus grave d'abord, sans perdre l'ordre de découverte à gravité égale.
    notes.sort_by_key(|n| std::cmp::Reverse(n.level));

    Assessment {
        footprint,
        cpu,
        memory,
        fit,
        notes,
    }
}

/// Rassemble les remarques sur une requête et sa confrontation au cluster.
fn collect_notes(
    req: &DeployRequest,
    footprint: &Footprint,
    cpu: Option<Axis>,
    memory: Option<Axis>,
    notes: &mut Vec<Note>,
) {
    // --- dépassement de capacité
    if let Some(axis) = cpu.filter(|a| a.after_ratio() > 1.0) {
        notes.push(Note::new(
            NoteLevel::Danger,
            format!(
                "Il manque {:.0} m de CPU : les pods resteront en attente d'un nœud capable de \
                 les accueillir.",
                -axis.remaining()
            ),
        ));
    }
    if let Some(axis) = memory.filter(|a| a.after_ratio() > 1.0) {
        notes.push(Note::new(
            NoteLevel::Danger,
            format!(
                "Il manque {} de mémoire : les pods resteront en attente d'un nœud capable de les \
                 accueillir.",
                human_bytes(-axis.remaining())
            ),
        ));
    }

    // --- demandes de ressources absentes
    if footprint.cpu_request_millis.is_none() || footprint.memory_request_bytes.is_none() {
        notes.push(Note::new(
            NoteLevel::Warning,
            "Sans demande de ressources, l'ordonnanceur place les pods à l'aveugle et ils seront \
             les premiers arrêtés quand un nœud manquera de mémoire.",
        ));
    }

    // --- limite inférieure à la demande : Kubernetes refuse l'objet
    if let (Some(r), Some(l)) = (
        req.cpu_request.as_deref().and_then(parse_cpu_millis),
        req.cpu_limit.as_deref().and_then(parse_cpu_millis),
    ) {
        if l < r {
            notes.push(Note::new(
                NoteLevel::Danger,
                "La limite CPU est inférieure à la demande : Kubernetes refusera le déploiement.",
            ));
        }
    }
    if let (Some(r), Some(l)) = (
        req.memory_request.as_deref().and_then(parse_memory_bytes),
        req.memory_limit.as_deref().and_then(parse_memory_bytes),
    ) {
        if l < r {
            notes.push(Note::new(
                NoteLevel::Danger,
                "La limite mémoire est inférieure à la demande : Kubernetes refusera le \
                 déploiement.",
            ));
        }
    }

    // --- volume persistant partagé par plusieurs répliques
    if let Some(pvc) = &req.pvc {
        let mode = pvc.access_mode.as_deref().unwrap_or("ReadWriteOnce").trim();
        if req.replicas > 1 && matches!(mode, "ReadWriteOnce" | "ReadWriteOncePod") {
            notes.push(Note::new(
                NoteLevel::Danger,
                format!(
                    "Le volume est en {mode} : une seule réplique pourra le monter, les {} autres \
                     resteront bloquées au démarrage.",
                    req.replicas - 1
                ),
            ));
        }
    }

    // --- nombre de répliques
    match req.replicas {
        0 => notes.push(Note::new(
            NoteLevel::Info,
            "Zéro réplique : les objets seront créés, mais aucun pod ne démarrera.",
        )),
        1 => notes.push(Note::new(
            NoteLevel::Info,
            "Une seule réplique : l'application sera interrompue pendant les redémarrages et les \
             mises à jour.",
        )),
        _ => {}
    }

    // --- tag mouvant
    let image = req.image.trim();
    if image.ends_with(":latest") || (!image.contains(':') && !image.contains('@')) {
        notes.push(Note::new(
            NoteLevel::Warning,
            "L'image vise « latest » : le contenu déployé changera sans prévenir. Épinglez une \
             version pour garder un déploiement reproductible.",
        ));
    }

    // --- exposition
    if req.ports.is_empty() {
        notes.push(Note::new(
            NoteLevel::Info,
            "Aucun port déclaré : aucun Service ne sera créé et l'application ne sera joignable \
             que depuis son propre pod.",
        ));
    }
    if req
        .ingress_host
        .as_deref()
        .map(str::trim)
        .is_some_and(|h| !h.is_empty())
        && req
            .ingress_class
            .as_deref()
            .map(str::trim)
            .is_none_or(str::is_empty)
    {
        notes.push(Note::new(
            NoteLevel::Info,
            "Aucune classe d'Ingress précisée : le cluster utilisera sa classe par défaut, s'il \
             en a une.",
        ));
    }

    // --- cluster serré sans dépassement
    let tight = [cpu, memory]
        .into_iter()
        .flatten()
        .any(|a| a.after_ratio() > TIGHT_FRACTION && a.after_ratio() <= 1.0);
    if tight {
        notes.push(Note::new(
            NoteLevel::Warning,
            "Le cluster dépassera 85 % d'occupation : il ne restera guère de marge pour absorber \
             un pic ou la perte d'un nœud.",
        ));
    }
}

/// Taille lisible en octets binaires, pour les textes de remarque.
fn human_bytes(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["o", "Kio", "Mio", "Gio", "Tio"];
    let mut value = bytes.abs();
    let mut i = 0usize;
    while value >= 1024.0 && i + 1 < UNITS.len() {
        value /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{value:.0} o")
    } else {
        format!("{value:.1} {}", UNITS[i])
    }
}

/// Taille en gibioctets d'une quantité Kubernetes, arrondie au plus proche.
pub fn gib_of(quantity: &str) -> Option<u32> {
    let bytes = parse_memory_bytes(quantity.trim())?;
    if bytes <= 0 {
        return None;
    }
    let gib = (bytes as f64 / GIB as f64).round();
    Some(gib.clamp(1.0, f64::from(u32::MAX)) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy::PvcSpec;

    fn base() -> DeployRequest {
        DeployRequest {
            name: "demo".to_string(),
            namespace: "default".to_string(),
            image: "nginx:1.27.3-alpine".to_string(),
            replicas: 1,
            ..DeployRequest::default()
        }
    }

    fn cluster(
        cpu_capacity: f64,
        cpu_used: f64,
        mem_capacity: i64,
        mem_used: i64,
    ) -> ClusterOverview {
        ClusterOverview {
            cpu_capacity_millis: cpu_capacity,
            cpu_used_millis: cpu_used,
            memory_capacity_bytes: mem_capacity,
            memory_used_bytes: mem_used,
            metrics_available: true,
            ..ClusterOverview::default()
        }
    }

    #[test]
    fn profils_aller_retour() {
        for profil in SizeProfile::PRESETS {
            let mut req = base();
            profil.apply_to(&mut req);
            assert_eq!(
                SizeProfile::detect(&req),
                profil,
                "le profil « {} » doit se reconnaître lui-même",
                profil.label()
            );
        }
    }

    #[test]
    fn profil_personnalise_non_reconnu() {
        let mut req = base();
        req.cpu_request = Some("77m".to_string());
        assert_eq!(SizeProfile::detect(&req), SizeProfile::Custom);
        // Une requête nue n'est pas un préréglage non plus.
        assert_eq!(SizeProfile::detect(&base()), SizeProfile::Custom);
    }

    #[test]
    fn detection_insensible_a_lecriture_des_quantites() {
        let mut req = base();
        // « Grand » vaut 1 cœur / 2 cœurs, écrits ici en millicores.
        req.cpu_request = Some("1000m".to_string());
        req.cpu_limit = Some("2000m".to_string());
        req.memory_request = Some("2048Mi".to_string());
        req.memory_limit = Some("4096Mi".to_string());
        assert_eq!(SizeProfile::detect(&req), SizeProfile::Large);
    }

    #[test]
    fn empreinte_multipliee_par_les_repliques() {
        let mut req = base();
        req.replicas = 3;
        SizeProfile::Small.apply_to(&mut req);
        let f = footprint(&req);
        assert_eq!(f.replicas, 3);
        assert_eq!(f.cpu_request_millis, Some(150.0));
        assert_eq!(f.memory_request_bytes, Some(3 * 128 * 1024 * 1024));
        assert_eq!(f.cpu_limit_millis, Some(1500.0));
    }

    #[test]
    fn stockage_non_multiplie_par_les_repliques() {
        let mut req = base();
        req.replicas = 4;
        req.pvc = Some(PvcSpec {
            size: "10Gi".to_string(),
            mount_path: "/data".to_string(),
            ..PvcSpec::default()
        });
        // Le générateur crée un seul PVC, monté par tous les pods.
        assert_eq!(footprint(&req).storage_bytes, Some(10 * GIB));
    }

    #[test]
    fn quantites_absentes_restent_inconnues() {
        let f = footprint(&base());
        assert_eq!(f.cpu_request_millis, None);
        assert_eq!(f.memory_request_bytes, None);
        assert_eq!(f.storage_bytes, None);
    }

    #[test]
    fn verdict_confortable() {
        let mut req = base();
        SizeProfile::Small.apply_to(&mut req);
        let a = assess(&req, Some(&cluster(8000.0, 2000.0, 16 * GIB, 4 * GIB)));
        assert_eq!(a.fit, Fit::Fits);
        let cpu = a.cpu.expect("axe CPU connu");
        assert!(cpu.before_fraction() < cpu.after_fraction());
        assert!(cpu.remaining() > 0.0);
    }

    #[test]
    fn verdict_serre_au_dela_de_85_pourcent() {
        let mut req = base();
        SizeProfile::Medium.apply_to(&mut req);
        req.replicas = 2;
        // 7 400 m occupés sur 8 000, plus 500 m demandés : 98,75 %.
        let a = assess(&req, Some(&cluster(8000.0, 7400.0, 64 * GIB, GIB)));
        assert_eq!(a.fit, Fit::Tight);
        assert!(a
            .notes
            .iter()
            .any(|n| n.level == NoteLevel::Warning && n.text.contains("85 %")));
    }

    #[test]
    fn verdict_depassement_et_manque_chiffre() {
        let mut req = base();
        SizeProfile::Large.apply_to(&mut req);
        req.replicas = 10;
        let a = assess(&req, Some(&cluster(4000.0, 1000.0, 8 * GIB, GIB)));
        assert_eq!(a.fit, Fit::Exceeds);
        assert!(a.notes.iter().any(|n| n.level == NoteLevel::Danger));
    }

    #[test]
    fn sans_metriques_le_verdict_est_inconnu() {
        let mut req = base();
        SizeProfile::Small.apply_to(&mut req);
        let muet = ClusterOverview {
            metrics_available: false,
            cpu_capacity_millis: 8000.0,
            ..ClusterOverview::default()
        };
        let a = assess(&req, Some(&muet));
        assert_eq!(a.fit, Fit::Unknown);
        assert!(a.cpu.is_none() && a.memory.is_none());
        // Les remarques indépendantes du cluster restent produites.
        assert!(!a.notes.is_empty());
        assert_eq!(assess(&req, None).fit, Fit::Unknown);
    }

    #[test]
    fn volume_partage_entre_repliques_signale() {
        let mut req = base();
        req.replicas = 3;
        req.pvc = Some(PvcSpec {
            size: "5Gi".to_string(),
            mount_path: "/data".to_string(),
            ..PvcSpec::default()
        });
        let a = assess(&req, None);
        assert!(a
            .notes
            .iter()
            .any(|n| n.level == NoteLevel::Danger && n.text.contains("ReadWriteOnce")));

        // En ReadWriteMany, les répliques peuvent partager le volume.
        req.pvc = Some(PvcSpec {
            size: "5Gi".to_string(),
            mount_path: "/data".to_string(),
            access_mode: Some("ReadWriteMany".to_string()),
            ..PvcSpec::default()
        });
        let a = assess(&req, None);
        assert!(!a
            .notes
            .iter()
            .any(|n| n.text.contains("resteront bloquées")));
    }

    #[test]
    fn limite_inferieure_a_la_demande_signalee() {
        let mut req = base();
        req.cpu_request = Some("500m".to_string());
        req.cpu_limit = Some("100m".to_string());
        req.memory_request = Some("1Gi".to_string());
        req.memory_limit = Some("256Mi".to_string());
        let a = assess(&req, None);
        let dangers = a
            .notes
            .iter()
            .filter(|n| n.level == NoteLevel::Danger)
            .count();
        assert_eq!(dangers, 2, "une remarque par axe fautif");
    }

    #[test]
    fn tag_mouvant_signale() {
        let mut req = base();
        req.image = "nginx".to_string();
        assert!(assess(&req, None)
            .notes
            .iter()
            .any(|n| n.text.contains("latest")));
        req.image = "nginx:latest".to_string();
        assert!(assess(&req, None)
            .notes
            .iter()
            .any(|n| n.text.contains("latest")));
        req.image = "nginx:1.27.3-alpine".to_string();
        assert!(!assess(&req, None)
            .notes
            .iter()
            .any(|n| n.text.contains("latest")));
    }

    #[test]
    fn remarques_triees_par_gravite() {
        let mut req = base();
        req.image = "nginx".to_string();
        req.replicas = 3;
        req.pvc = Some(PvcSpec {
            size: "5Gi".to_string(),
            mount_path: "/data".to_string(),
            ..PvcSpec::default()
        });
        let a = assess(&req, None);
        let niveaux: Vec<NoteLevel> = a.notes.iter().map(|n| n.level).collect();
        let mut tries = niveaux.clone();
        tries.sort_by(|x, y| y.cmp(x));
        assert_eq!(niveaux, tries, "les remarques graves viennent en tête");
    }

    #[test]
    fn profils_conseilles_par_categorie() {
        let base_de_donnees = crate::catalog::find_app("postgresql").expect("postgresql présent");
        assert_eq!(recommended_profile(base_de_donnees), SizeProfile::Medium);
        let serveur_web = crate::catalog::find_app("nginx").expect("nginx présent");
        assert_eq!(recommended_profile(serveur_web), SizeProfile::Small);
    }

    #[test]
    fn conversion_en_gibioctets() {
        assert_eq!(gib_of("10Gi"), Some(10));
        assert_eq!(gib_of("1024Mi"), Some(1));
        assert_eq!(
            gib_of("500Mi"),
            Some(1),
            "arrondi au gibioctet le plus proche"
        );
        assert_eq!(gib_of(""), None);
        assert_eq!(gib_of("bizarre"), None);
    }

    #[test]
    fn axe_borne_les_fractions() {
        let axis = Axis {
            used: 6000.0,
            added: 4000.0,
            capacity: 8000.0,
        };
        assert_eq!(
            axis.after_fraction(),
            1.0,
            "la barre ne dépasse jamais 100 %"
        );
        assert!(
            axis.after_ratio() > 1.0,
            "le ratio, lui, dit le dépassement"
        );
        assert!(axis.remaining() < 0.0);
        let vide = Axis::default();
        assert_eq!(vide.before_fraction(), 0.0);
        assert_eq!(vide.after_ratio(), 0.0);
    }
}
