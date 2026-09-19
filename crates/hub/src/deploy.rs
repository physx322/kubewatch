//! Générateur de manifestes Kubernetes.
//!
//! À partir d'une description simple ([`DeployRequest`]), produit un document YAML
//! multi-objets prêt à appliquer : `Deployment`, puis `Service`, `Ingress` et
//! `PersistentVolumeClaim` selon ce qui est demandé.
//!
//! Le YAML n'est jamais construit par concaténation de chaînes : les objets sont assemblés
//! en JSON puis sérialisés, ce qui rend impossible toute injection ou indentation erronée.

use crate::error::{Error, Result};
use crate::model::ImageRef;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Étiquette identifiant l'application déployée.
const LABEL_NAME: &str = "app.kubernetes.io/name";

/// Étiquette identifiant l'instance déployée.
const LABEL_INSTANCE: &str = "app.kubernetes.io/instance";

/// Étiquette signalant que l'objet est géré par KubeWatch.
const LABEL_MANAGED_BY: &str = "app.kubernetes.io/managed-by";

/// Valeur de l'étiquette de gestion.
const MANAGED_BY: &str = "kubewatch";

/// Nom du volume monté lorsqu'un PVC est demandé.
const VOLUME_NAME: &str = "data";

/// Types de Service acceptés.
const SERVICE_TYPES: &[&str] = &["ClusterIP", "NodePort", "LoadBalancer"];

/// Modes d'accès acceptés pour un volume persistant.
const ACCESS_MODES: &[&str] = &[
    "ReadWriteOnce",
    "ReadOnlyMany",
    "ReadWriteMany",
    "ReadWriteOncePod",
];

/// Étiquette DNS-1123 : minuscules, chiffres et tirets, bornes alphanumériques.
static DNS1123_LABEL: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-z0-9]([-a-z0-9]*[a-z0-9])?$").expect("motif DNS-1123 valide"));

/// Nom de port de Service : au plus 15 caractères et au moins une lettre.
static PORT_NAME: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-z0-9]([-a-z0-9]*[a-z0-9])?$").expect("motif de nom de port valide"));

/// Quantité Kubernetes : `100m`, `1`, `2Gi`, `500Mi`, `1e3`.
static QUANTITY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?(?:m|k|M|G|T|P|E|Ki|Mi|Gi|Ti|Pi|Ei)?$")
        .expect("motif de quantité valide")
});

/// Nom de variable d'environnement POSIX.
static ENV_NAME: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("motif de variable valide"));

/// Nom d'hôte DNS, avec joker de sous-domaine facultatif.
static HOSTNAME: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?:\*\.)?(?:[a-zA-Z0-9](?:[-a-zA-Z0-9]*[a-zA-Z0-9])?\.)*[a-zA-Z0-9](?:[-a-zA-Z0-9]*[a-zA-Z0-9])?$")
        .expect("motif de nom d'hôte valide")
});

/// Partie « nom » d'une clé d'étiquette ou d'annotation.
static LABEL_KEY_NAME: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^[A-Za-z0-9]([-A-Za-z0-9_.]*[A-Za-z0-9])?$").expect("motif de clé valide")
});

/// Port exposé par le conteneur, et éventuellement par le Service.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PortSpec {
    /// Nom du port, repris tel quel par le Service et l'Ingress.
    pub name: Option<String>,
    /// Port écouté à l'intérieur du conteneur.
    pub container_port: u16,
    /// Port exposé par le Service ; identique au port conteneur par défaut.
    pub service_port: Option<u16>,
    /// `TCP` (défaut), `UDP` ou `SCTP`.
    pub protocol: Option<String>,
}

impl PortSpec {
    /// Protocole normalisé en majuscules.
    fn protocol_value(&self) -> String {
        self.protocol
            .as_deref()
            .map(|p| p.trim().to_ascii_uppercase())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "TCP".to_string())
    }

    /// Port exposé par le Service.
    fn service_port_value(&self) -> u16 {
        self.service_port.unwrap_or(self.container_port)
    }

    /// Nom effectif du port, généré si l'utilisateur n'en fournit pas.
    fn name_value(&self) -> String {
        match self.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => n.to_ascii_lowercase(),
            // Un nom de port doit contenir au moins une lettre et tenir en 15 caractères.
            None => format!("p{}", self.container_port),
        }
    }
}

/// Volume persistant demandé par l'application.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PvcSpec {
    /// Nom du PersistentVolumeClaim ; dérivé du nom de l'application par défaut.
    pub name: Option<String>,
    /// Taille demandée, sous forme de quantité Kubernetes (`10Gi`).
    pub size: String,
    /// Point de montage absolu dans le conteneur.
    pub mount_path: String,
    /// Classe de stockage ; celle par défaut du cluster si absente.
    pub storage_class: Option<String>,
    /// Mode d'accès ; `ReadWriteOnce` par défaut.
    pub access_mode: Option<String>,
}

impl PvcSpec {
    /// Nom effectif du PersistentVolumeClaim.
    fn name_value(&self, app: &str) -> String {
        match self.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => n.to_string(),
            None => format!("{app}-data"),
        }
    }

    /// Mode d'accès effectif.
    fn access_mode_value(&self) -> String {
        self.access_mode
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .unwrap_or("ReadWriteOnce")
            .to_string()
    }
}

/// Description complète d'un déploiement à générer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DeployRequest {
    /// Nom de l'application ; sert de nom d'objet et d'étiquette.
    pub name: String,
    /// Namespace cible.
    pub namespace: String,
    /// Référence complète de l'image à déployer.
    pub image: String,
    /// Nombre de répliques souhaitées.
    pub replicas: i32,
    /// Ports exposés par le conteneur.
    pub ports: Vec<PortSpec>,
    /// Variables d'environnement, sous forme de couples clé/valeur.
    pub env: Vec<(String, String)>,
    /// Demande CPU (`100m`, `0.5`…).
    pub cpu_request: Option<String>,
    /// Limite CPU.
    pub cpu_limit: Option<String>,
    /// Demande mémoire (`256Mi`, `1Gi`…).
    pub memory_request: Option<String>,
    /// Limite mémoire.
    pub memory_limit: Option<String>,
    /// Type de Service : `ClusterIP` (défaut), `NodePort` ou `LoadBalancer`.
    pub service_type: Option<String>,
    /// Nom d'hôte de l'Ingress ; aucun Ingress n'est généré s'il est absent.
    pub ingress_host: Option<String>,
    /// Classe d'Ingress (`nginx`, `traefik`…).
    pub ingress_class: Option<String>,
    /// Secret TLS à utiliser pour l'Ingress.
    pub ingress_tls_secret: Option<String>,
    /// Secret d'authentification au registre d'images.
    pub image_pull_secret: Option<String>,
    /// Volume persistant à créer et monter.
    pub pvc: Option<PvcSpec>,
    /// Étiquettes supplémentaires appliquées à tous les objets.
    pub labels: BTreeMap<String, String>,
    /// Annotations supplémentaires appliquées à tous les objets.
    pub annotations: BTreeMap<String, String>,
    /// Remplace le point d'entrée de l'image.
    pub command: Vec<String>,
    /// Remplace les arguments de l'image.
    pub args: Vec<String>,
    /// Compte de service utilisé par les pods.
    pub service_account: Option<String>,
    /// Contraintes de placement sur les nœuds.
    pub node_selector: BTreeMap<String, String>,
}

impl Default for DeployRequest {
    fn default() -> Self {
        DeployRequest {
            name: String::new(),
            namespace: "default".to_string(),
            image: String::new(),
            // Une réplique est le choix le plus sûr pour un premier déploiement.
            replicas: 1,
            ports: Vec::new(),
            env: Vec::new(),
            cpu_request: None,
            cpu_limit: None,
            memory_request: None,
            memory_limit: None,
            service_type: None,
            ingress_host: None,
            ingress_class: None,
            ingress_tls_secret: None,
            image_pull_secret: None,
            pvc: None,
            labels: BTreeMap::new(),
            annotations: BTreeMap::new(),
            command: Vec::new(),
            args: Vec::new(),
            service_account: None,
            node_selector: BTreeMap::new(),
        }
    }
}

impl DeployRequest {
    /// Type de Service effectif.
    fn service_type_value(&self) -> String {
        self.service_type
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or("ClusterIP")
            .to_string()
    }
}

// ================================================================== validation

/// Vrai si la chaîne est une étiquette DNS-1123 valide (63 caractères au plus).
pub fn is_dns1123_label(value: &str) -> bool {
    !value.is_empty() && value.len() <= 63 && DNS1123_LABEL.is_match(value)
}

/// Vrai si la chaîne est une quantité Kubernetes strictement positive.
pub fn is_quantity(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() || !QUANTITY.is_match(value) {
        return false;
    }
    // Une demande de ressource nulle n'a pas de sens et masque souvent une erreur de saisie.
    let digits: String = value
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    digits.parse::<f64>().map(|n| n > 0.0).unwrap_or(false)
}

/// Vérifie qu'une requête de déploiement est applicable telle quelle.
///
/// Toutes les erreurs sont en français et désignent précisément le champ fautif.
pub fn validate(req: &DeployRequest) -> Result<()> {
    if !is_dns1123_label(&req.name) {
        return Err(Error::Invalid(format!(
            "nom d'application « {} » invalide : utilisez uniquement des minuscules, des chiffres \
             et des tirets, en commençant et finissant par un caractère alphanumérique \
             (63 caractères au plus). Exemple : « mon-api ».",
            req.name
        )));
    }
    if !is_dns1123_label(&req.namespace) {
        return Err(Error::Invalid(format!(
            "namespace « {} » invalide : il doit respecter le format DNS-1123 \
             (minuscules, chiffres et tirets). Exemple : « production ».",
            req.namespace
        )));
    }

    let image = req.image.trim();
    if image.is_empty() {
        return Err(Error::Invalid(
            "image manquante : indiquez une référence complète, par exemple « nginx:1.27.3-alpine »."
                .to_string(),
        ));
    }
    ImageRef::parse(image).map_err(|e| {
        Error::Invalid(format!("image « {image} » inutilisable : {e}"))
    })?;

    if !(0..=1000).contains(&req.replicas) {
        return Err(Error::Invalid(format!(
            "nombre de répliques invalide ({}) : indiquez une valeur entre 0 et 1000.",
            req.replicas
        )));
    }

    // --- ports
    let mut seen_container: Vec<u16> = Vec::new();
    let mut seen_service: Vec<u16> = Vec::new();
    let mut seen_names: Vec<String> = Vec::new();
    for port in &req.ports {
        if port.container_port == 0 {
            return Err(Error::Invalid(
                "port conteneur manquant ou nul : indiquez une valeur entre 1 et 65535."
                    .to_string(),
            ));
        }
        if seen_container.contains(&port.container_port) {
            return Err(Error::Invalid(format!(
                "le port conteneur {} est déclaré deux fois : chaque port doit être unique.",
                port.container_port
            )));
        }
        seen_container.push(port.container_port);

        if let Some(service_port) = port.service_port {
            if service_port == 0 {
                return Err(Error::Invalid(format!(
                    "port de service nul pour le port conteneur {} : indiquez une valeur entre 1 \
                     et 65535.",
                    port.container_port
                )));
            }
        }
        let service_port = port.service_port_value();
        if seen_service.contains(&service_port) {
            return Err(Error::Invalid(format!(
                "le port de service {service_port} est déclaré deux fois : un Service ne peut pas \
                 exposer deux fois le même port."
            )));
        }
        seen_service.push(service_port);

        let protocol = port.protocol_value();
        if !["TCP", "UDP", "SCTP"].contains(&protocol.as_str()) {
            return Err(Error::Invalid(format!(
                "protocole « {protocol} » invalide : les valeurs acceptées sont TCP, UDP et SCTP."
            )));
        }

        let name = port.name_value();
        if name.len() > 15 || !PORT_NAME.is_match(&name) || !name.chars().any(|c| c.is_alphabetic())
        {
            return Err(Error::Invalid(format!(
                "nom de port « {name} » invalide : 15 caractères au plus, en minuscules, avec au \
                 moins une lettre. Exemple : « http »."
            )));
        }
        if seen_names.contains(&name) {
            return Err(Error::Invalid(format!(
                "le nom de port « {name} » est utilisé deux fois : chaque port doit avoir un nom \
                 distinct."
            )));
        }
        seen_names.push(name);
    }

    // --- variables d'environnement
    for (key, _) in &req.env {
        if !ENV_NAME.is_match(key) {
            return Err(Error::Invalid(format!(
                "variable d'environnement « {key} » invalide : le nom doit commencer par une \
                 lettre ou « _ » et ne contenir que des lettres, chiffres et « _ »."
            )));
        }
    }

    // --- ressources
    for (label, value) in [
        ("cpuRequest", &req.cpu_request),
        ("cpuLimit", &req.cpu_limit),
        ("memoryRequest", &req.memory_request),
        ("memoryLimit", &req.memory_limit),
    ] {
        if let Some(v) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            if !is_quantity(v) {
                return Err(Error::Invalid(format!(
                    "quantité « {v} » invalide pour {label} : utilisez le format Kubernetes, par \
                     exemple « 100m », « 0.5 », « 256Mi » ou « 2Gi »."
                )));
            }
        }
    }

    // --- service
    let service_type = req.service_type_value();
    if !SERVICE_TYPES.contains(&service_type.as_str()) {
        return Err(Error::Invalid(format!(
            "type de Service « {service_type} » invalide : les valeurs acceptées sont {}.",
            SERVICE_TYPES.join(", ")
        )));
    }

    // --- ingress
    if let Some(host) = req.ingress_host.as_deref().map(str::trim).filter(|h| !h.is_empty()) {
        if host.len() > 253 || !HOSTNAME.is_match(host) {
            return Err(Error::Invalid(format!(
                "nom d'hôte « {host} » invalide : indiquez un nom DNS, par exemple \
                 « api.exemple.fr »."
            )));
        }
        if req.ports.is_empty() {
            return Err(Error::Invalid(
                "un Ingress a été demandé mais aucun port n'est exposé : ajoutez au moins un port \
                 conteneur pour que le trafic puisse être routé."
                    .to_string(),
            ));
        }
    }
    check_optional_label("classe d'Ingress", &req.ingress_class)?;
    check_optional_label("secret TLS d'Ingress", &req.ingress_tls_secret)?;
    check_optional_label("secret de registre", &req.image_pull_secret)?;
    check_optional_label("compte de service", &req.service_account)?;

    // --- volume persistant
    if let Some(pvc) = &req.pvc {
        let size = pvc.size.trim();
        if size.is_empty() {
            return Err(Error::Invalid(
                "taille de volume manquante : indiquez par exemple « 10Gi ».".to_string(),
            ));
        }
        if !is_quantity(size) {
            return Err(Error::Invalid(format!(
                "taille de volume « {size} » invalide : utilisez une quantité Kubernetes, par \
                 exemple « 10Gi » ou « 500Mi »."
            )));
        }
        let mount_path = pvc.mount_path.trim();
        if !mount_path.starts_with('/') {
            return Err(Error::Invalid(format!(
                "point de montage « {mount_path} » invalide : il doit être un chemin absolu, par \
                 exemple « /var/lib/données »."
            )));
        }
        if mount_path == "/" {
            return Err(Error::Invalid(
                "point de montage « / » refusé : monter un volume sur la racine rend le conteneur \
                 inutilisable."
                    .to_string(),
            ));
        }
        let name = pvc.name_value(&req.name);
        if !is_dns1123_label(&name) {
            return Err(Error::Invalid(format!(
                "nom de volume « {name} » invalide : il doit respecter le format DNS-1123."
            )));
        }
        check_optional_label("classe de stockage", &pvc.storage_class)?;
        let mode = pvc.access_mode_value();
        if !ACCESS_MODES.contains(&mode.as_str()) {
            return Err(Error::Invalid(format!(
                "mode d'accès « {mode} » invalide : les valeurs acceptées sont {}.",
                ACCESS_MODES.join(", ")
            )));
        }
    }

    // --- étiquettes, annotations et placement
    for key in req.labels.keys().chain(req.annotations.keys()) {
        check_metadata_key(key)?;
    }
    for key in req.node_selector.keys() {
        check_metadata_key(key)?;
    }

    Ok(())
}

/// Vérifie qu'un champ facultatif, lorsqu'il est renseigné, est une étiquette DNS-1123.
fn check_optional_label(field: &str, value: &Option<String>) -> Result<()> {
    if let Some(v) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        if !is_dns1123_label(v) {
            return Err(Error::Invalid(format!(
                "{field} « {v} » invalide : utilisez des minuscules, des chiffres et des tirets \
                 (63 caractères au plus)."
            )));
        }
    }
    Ok(())
}

/// Vérifie la forme d'une clé d'étiquette, d'annotation ou de sélecteur de nœud.
fn check_metadata_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > 253 {
        return Err(Error::Invalid(format!(
            "clé « {key} » invalide : elle doit contenir entre 1 et 253 caractères."
        )));
    }
    let name = match key.split_once('/') {
        Some((prefix, name)) => {
            if prefix.is_empty() || !HOSTNAME.is_match(prefix) {
                return Err(Error::Invalid(format!(
                    "préfixe de clé « {prefix} » invalide : il doit être un nom DNS, par exemple \
                     « app.kubernetes.io »."
                )));
            }
            name
        }
        None => key,
    };
    if name.is_empty() || name.len() > 63 || !LABEL_KEY_NAME.is_match(name) {
        return Err(Error::Invalid(format!(
            "clé « {key} » invalide : la partie après « / » doit contenir au plus 63 caractères \
             alphanumériques, tirets, points ou soulignés."
        )));
    }
    Ok(())
}

// ================================================================== génération

/// Produit le document YAML multi-objets correspondant à la requête.
pub fn render_manifests(req: &DeployRequest) -> Result<String> {
    // Générer un manifeste invalide serait pire que refuser la demande.
    validate(req)?;

    let mut documents: Vec<Value> = Vec::with_capacity(4);
    documents.push(build_deployment(req));
    if !req.ports.is_empty() {
        documents.push(build_service(req));
    }
    if req.ingress_host.as_deref().map(str::trim).is_some_and(|h| !h.is_empty()) {
        documents.push(build_ingress(req));
    }
    if let Some(pvc) = &req.pvc {
        documents.push(build_pvc(req, pvc));
    }

    let mut out = String::from("# Manifestes générés par KubeWatch — modifiables avant application\n");
    for (index, document) in documents.iter().enumerate() {
        if index > 0 {
            out.push_str("---\n");
        }
        out.push_str(&serde_yaml_ng::to_string(document)?);
    }
    Ok(out)
}

/// Étiquettes de sélection : elles sont immuables une fois le Deployment créé.
fn selector_labels(req: &DeployRequest) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert(LABEL_NAME.to_string(), Value::String(req.name.clone()));
    map.insert(LABEL_INSTANCE.to_string(), Value::String(req.name.clone()));
    map
}

/// Étiquettes complètes : sélection, gestion par KubeWatch, puis ajouts de l'utilisateur.
fn all_labels(req: &DeployRequest) -> Map<String, Value> {
    let mut map = selector_labels(req);
    map.insert(LABEL_MANAGED_BY.to_string(), Value::String(MANAGED_BY.to_string()));
    for (key, value) in &req.labels {
        map.insert(key.clone(), Value::String(value.clone()));
    }
    map
}

/// Métadonnées communes à tous les objets générés.
fn metadata(req: &DeployRequest, name: &str) -> Value {
    let mut meta = Map::new();
    meta.insert("name".to_string(), Value::String(name.to_string()));
    meta.insert("namespace".to_string(), Value::String(req.namespace.clone()));
    meta.insert("labels".to_string(), Value::Object(all_labels(req)));
    if !req.annotations.is_empty() {
        let mut annotations = Map::new();
        for (key, value) in &req.annotations {
            annotations.insert(key.clone(), Value::String(value.clone()));
        }
        meta.insert("annotations".to_string(), Value::Object(annotations));
    }
    Value::Object(meta)
}

/// Construit le conteneur principal.
fn build_container(req: &DeployRequest) -> Value {
    let mut container = Map::new();
    container.insert("name".to_string(), Value::String(req.name.clone()));
    container.insert("image".to_string(), Value::String(req.image.trim().to_string()));
    container.insert(
        "imagePullPolicy".to_string(),
        Value::String("IfNotPresent".to_string()),
    );

    if !req.command.is_empty() {
        container.insert(
            "command".to_string(),
            Value::Array(req.command.iter().map(|c| Value::String(c.clone())).collect()),
        );
    }
    if !req.args.is_empty() {
        container.insert(
            "args".to_string(),
            Value::Array(req.args.iter().map(|a| Value::String(a.clone())).collect()),
        );
    }

    if !req.ports.is_empty() {
        let ports: Vec<Value> = req
            .ports
            .iter()
            .map(|port| {
                let mut p = Map::new();
                p.insert("name".to_string(), Value::String(port.name_value()));
                p.insert(
                    "containerPort".to_string(),
                    Value::Number(port.container_port.into()),
                );
                p.insert("protocol".to_string(), Value::String(port.protocol_value()));
                Value::Object(p)
            })
            .collect();
        container.insert("ports".to_string(), Value::Array(ports));
    }

    if !req.env.is_empty() {
        let env: Vec<Value> = req
            .env
            .iter()
            .map(|(key, value)| {
                let mut e = Map::new();
                e.insert("name".to_string(), Value::String(key.clone()));
                e.insert("value".to_string(), Value::String(value.clone()));
                Value::Object(e)
            })
            .collect();
        container.insert("env".to_string(), Value::Array(env));
    }

    if let Some(resources) = build_resources(req) {
        container.insert("resources".to_string(), resources);
    }

    // Sondes : le premier port TCP sert de signal de disponibilité. On utilise `tcpSocket`
    // plutôt que `httpGet`, car le chemin de santé de l'application est inconnu du
    // générateur et une sonde HTTP sur « / » empêcherait souvent le pod de devenir prêt.
    if let Some(port) = req.ports.iter().find(|p| p.protocol_value() == "TCP") {
        let probe = |initial_delay: u64, period: u64| -> Value {
            let mut socket = Map::new();
            socket.insert("port".to_string(), Value::String(port.name_value()));
            let mut probe = Map::new();
            probe.insert("tcpSocket".to_string(), Value::Object(socket));
            probe.insert(
                "initialDelaySeconds".to_string(),
                Value::Number(initial_delay.into()),
            );
            probe.insert("periodSeconds".to_string(), Value::Number(period.into()));
            probe.insert("timeoutSeconds".to_string(), Value::Number(3u64.into()));
            probe.insert("failureThreshold".to_string(), Value::Number(3u64.into()));
            Value::Object(probe)
        };
        container.insert("readinessProbe".to_string(), probe(5, 10));
        container.insert("livenessProbe".to_string(), probe(20, 20));
    }

    if let Some(pvc) = &req.pvc {
        let mut mount = Map::new();
        mount.insert("name".to_string(), Value::String(VOLUME_NAME.to_string()));
        mount.insert(
            "mountPath".to_string(),
            Value::String(pvc.mount_path.trim().to_string()),
        );
        container.insert("volumeMounts".to_string(), Value::Array(vec![Value::Object(mount)]));
    }

    // Durcissement par défaut : ni élévation de privilèges, ni capacités superflues.
    let mut security = Map::new();
    security.insert("allowPrivilegeEscalation".to_string(), Value::Bool(false));
    let mut capabilities = Map::new();
    capabilities.insert(
        "drop".to_string(),
        Value::Array(vec![Value::String("ALL".to_string())]),
    );
    security.insert("capabilities".to_string(), Value::Object(capabilities));
    container.insert("securityContext".to_string(), Value::Object(security));

    Value::Object(container)
}

/// Construit le bloc `resources`, s'il y a quelque chose à déclarer.
fn build_resources(req: &DeployRequest) -> Option<Value> {
    let mut requests = Map::new();
    let mut limits = Map::new();
    let clean = |v: &Option<String>| -> Option<String> {
        v.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_string())
    };
    if let Some(v) = clean(&req.cpu_request) {
        requests.insert("cpu".to_string(), Value::String(v));
    }
    if let Some(v) = clean(&req.memory_request) {
        requests.insert("memory".to_string(), Value::String(v));
    }
    if let Some(v) = clean(&req.cpu_limit) {
        limits.insert("cpu".to_string(), Value::String(v));
    }
    if let Some(v) = clean(&req.memory_limit) {
        limits.insert("memory".to_string(), Value::String(v));
    }
    if requests.is_empty() && limits.is_empty() {
        return None;
    }
    let mut resources = Map::new();
    if !requests.is_empty() {
        resources.insert("requests".to_string(), Value::Object(requests));
    }
    if !limits.is_empty() {
        resources.insert("limits".to_string(), Value::Object(limits));
    }
    Some(Value::Object(resources))
}

/// Construit le `Deployment`.
fn build_deployment(req: &DeployRequest) -> Value {
    let mut pod_spec = Map::new();
    if let Some(sa) = req.service_account.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        pod_spec.insert("serviceAccountName".to_string(), Value::String(sa.to_string()));
    }
    if let Some(secret) = req
        .image_pull_secret
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let mut entry = Map::new();
        entry.insert("name".to_string(), Value::String(secret.to_string()));
        pod_spec.insert(
            "imagePullSecrets".to_string(),
            Value::Array(vec![Value::Object(entry)]),
        );
    }
    if !req.node_selector.is_empty() {
        let mut selector = Map::new();
        for (key, value) in &req.node_selector {
            selector.insert(key.clone(), Value::String(value.clone()));
        }
        pod_spec.insert("nodeSelector".to_string(), Value::Object(selector));
    }
    if let Some(pvc) = &req.pvc {
        let mut claim = Map::new();
        claim.insert(
            "claimName".to_string(),
            Value::String(pvc.name_value(&req.name)),
        );
        let mut volume = Map::new();
        volume.insert("name".to_string(), Value::String(VOLUME_NAME.to_string()));
        volume.insert("persistentVolumeClaim".to_string(), Value::Object(claim));
        pod_spec.insert("volumes".to_string(), Value::Array(vec![Value::Object(volume)]));
    }
    pod_spec.insert(
        "containers".to_string(),
        Value::Array(vec![build_container(req)]),
    );

    let mut pod_metadata = Map::new();
    pod_metadata.insert("labels".to_string(), Value::Object(all_labels(req)));
    if !req.annotations.is_empty() {
        let mut annotations = Map::new();
        for (key, value) in &req.annotations {
            annotations.insert(key.clone(), Value::String(value.clone()));
        }
        pod_metadata.insert("annotations".to_string(), Value::Object(annotations));
    }

    let mut template = Map::new();
    template.insert("metadata".to_string(), Value::Object(pod_metadata));
    template.insert("spec".to_string(), Value::Object(pod_spec));

    let mut rolling = Map::new();
    rolling.insert("maxSurge".to_string(), Value::Number(1u64.into()));
    rolling.insert("maxUnavailable".to_string(), Value::Number(0u64.into()));
    let mut strategy = Map::new();
    strategy.insert("type".to_string(), Value::String("RollingUpdate".to_string()));
    strategy.insert("rollingUpdate".to_string(), Value::Object(rolling));

    let mut selector = Map::new();
    selector.insert("matchLabels".to_string(), Value::Object(selector_labels(req)));

    let mut spec = Map::new();
    spec.insert("replicas".to_string(), Value::Number(req.replicas.into()));
    spec.insert("revisionHistoryLimit".to_string(), Value::Number(5u64.into()));
    spec.insert("selector".to_string(), Value::Object(selector));
    spec.insert("strategy".to_string(), Value::Object(strategy));
    spec.insert("template".to_string(), Value::Object(template));

    let mut doc = Map::new();
    doc.insert("apiVersion".to_string(), Value::String("apps/v1".to_string()));
    doc.insert("kind".to_string(), Value::String("Deployment".to_string()));
    doc.insert("metadata".to_string(), metadata(req, &req.name));
    doc.insert("spec".to_string(), Value::Object(spec));
    Value::Object(doc)
}

/// Construit le `Service` exposant les ports déclarés.
fn build_service(req: &DeployRequest) -> Value {
    let ports: Vec<Value> = req
        .ports
        .iter()
        .map(|port| {
            let mut p = Map::new();
            p.insert("name".to_string(), Value::String(port.name_value()));
            p.insert("port".to_string(), Value::Number(port.service_port_value().into()));
            p.insert("targetPort".to_string(), Value::String(port.name_value()));
            p.insert("protocol".to_string(), Value::String(port.protocol_value()));
            Value::Object(p)
        })
        .collect();

    let mut spec = Map::new();
    spec.insert("type".to_string(), Value::String(req.service_type_value()));
    spec.insert("selector".to_string(), Value::Object(selector_labels(req)));
    spec.insert("ports".to_string(), Value::Array(ports));

    let mut doc = Map::new();
    doc.insert("apiVersion".to_string(), Value::String("v1".to_string()));
    doc.insert("kind".to_string(), Value::String("Service".to_string()));
    doc.insert("metadata".to_string(), metadata(req, &req.name));
    doc.insert("spec".to_string(), Value::Object(spec));
    Value::Object(doc)
}

/// Construit l'`Ingress` routant le nom d'hôte vers le Service.
fn build_ingress(req: &DeployRequest) -> Value {
    let host = req.ingress_host.as_deref().unwrap_or_default().trim().to_string();
    let service_port = req
        .ports
        .first()
        .map(|p| p.service_port_value())
        .unwrap_or(80);

    let mut port = Map::new();
    port.insert("number".to_string(), Value::Number(service_port.into()));
    let mut service = Map::new();
    service.insert("name".to_string(), Value::String(req.name.clone()));
    service.insert("port".to_string(), Value::Object(port));
    let mut backend = Map::new();
    backend.insert("service".to_string(), Value::Object(service));

    let mut path = Map::new();
    path.insert("path".to_string(), Value::String("/".to_string()));
    path.insert("pathType".to_string(), Value::String("Prefix".to_string()));
    path.insert("backend".to_string(), Value::Object(backend));

    let mut http = Map::new();
    http.insert("paths".to_string(), Value::Array(vec![Value::Object(path)]));
    let mut rule = Map::new();
    rule.insert("host".to_string(), Value::String(host.clone()));
    rule.insert("http".to_string(), Value::Object(http));

    let mut spec = Map::new();
    if let Some(class) = req.ingress_class.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        spec.insert(
            "ingressClassName".to_string(),
            Value::String(class.to_string()),
        );
    }
    if let Some(secret) = req
        .ingress_tls_secret
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let mut tls = Map::new();
        tls.insert(
            "hosts".to_string(),
            Value::Array(vec![Value::String(host.clone())]),
        );
        tls.insert("secretName".to_string(), Value::String(secret.to_string()));
        spec.insert("tls".to_string(), Value::Array(vec![Value::Object(tls)]));
    }
    spec.insert("rules".to_string(), Value::Array(vec![Value::Object(rule)]));

    let mut doc = Map::new();
    doc.insert(
        "apiVersion".to_string(),
        Value::String("networking.k8s.io/v1".to_string()),
    );
    doc.insert("kind".to_string(), Value::String("Ingress".to_string()));
    doc.insert("metadata".to_string(), metadata(req, &req.name));
    doc.insert("spec".to_string(), Value::Object(spec));
    Value::Object(doc)
}

/// Construit le `PersistentVolumeClaim` monté par le conteneur.
fn build_pvc(req: &DeployRequest, pvc: &PvcSpec) -> Value {
    let mut requests = Map::new();
    requests.insert(
        "storage".to_string(),
        Value::String(pvc.size.trim().to_string()),
    );
    let mut resources = Map::new();
    resources.insert("requests".to_string(), Value::Object(requests));

    let mut spec = Map::new();
    spec.insert(
        "accessModes".to_string(),
        Value::Array(vec![Value::String(pvc.access_mode_value())]),
    );
    spec.insert("resources".to_string(), Value::Object(resources));
    if let Some(class) = pvc.storage_class.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        spec.insert(
            "storageClassName".to_string(),
            Value::String(class.to_string()),
        );
    }

    let mut doc = Map::new();
    doc.insert("apiVersion".to_string(), Value::String("v1".to_string()));
    doc.insert(
        "kind".to_string(),
        Value::String("PersistentVolumeClaim".to_string()),
    );
    doc.insert(
        "metadata".to_string(),
        metadata(req, &pvc.name_value(&req.name)),
    );
    doc.insert("spec".to_string(), Value::Object(spec));
    Value::Object(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    /// Requête minimale mais complète, servant de base aux variations des tests.
    fn base() -> DeployRequest {
        DeployRequest {
            name: "mon-api".to_string(),
            namespace: "production".to_string(),
            image: "ghcr.io/exemple/api:1.4.2".to_string(),
            replicas: 2,
            ports: vec![PortSpec {
                name: Some("http".to_string()),
                container_port: 8080,
                service_port: Some(80),
                protocol: None,
            }],
            env: vec![("LOG_LEVEL".to_string(), "info".to_string())],
            ..Default::default()
        }
    }

    /// Redécoupe un document multi-YAML en objets JSON, en sautant les documents vides.
    fn documents(yaml: &str) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        for doc in serde_yaml_ng::Deserializer::from_str(yaml) {
            let value = serde_yaml_ng::Value::deserialize(doc).expect("document YAML valide");
            if matches!(value, serde_yaml_ng::Value::Null) {
                continue;
            }
            out.push(serde_json::to_value(&value).expect("conversion JSON"));
        }
        out
    }

    fn kinds(yaml: &str) -> Vec<String> {
        documents(yaml)
            .iter()
            .filter_map(|d| d.get("kind").and_then(|k| k.as_str()).map(|s| s.to_string()))
            .collect()
    }

    #[test]
    fn requete_de_base_valide() {
        assert!(validate(&base()).is_ok());
    }

    #[test]
    fn deploiement_minimal_produit_deployment_et_service() {
        let yaml = render_manifests(&base()).expect("rendu réussi");
        assert_eq!(kinds(&yaml), vec!["Deployment", "Service"]);
    }

    #[test]
    fn sans_port_seul_le_deployment_est_genere() {
        let mut req = base();
        req.ports.clear();
        let yaml = render_manifests(&req).expect("rendu réussi");
        assert_eq!(kinds(&yaml), vec!["Deployment"]);
    }

    #[test]
    fn quatre_documents_avec_ingress_et_volume() {
        let mut req = base();
        req.ingress_host = Some("api.exemple.fr".to_string());
        req.ingress_class = Some("nginx".to_string());
        req.ingress_tls_secret = Some("api-tls".to_string());
        req.pvc = Some(PvcSpec {
            name: None,
            size: "10Gi".to_string(),
            mount_path: "/var/lib/api".to_string(),
            storage_class: Some("fast".to_string()),
            access_mode: None,
        });
        let yaml = render_manifests(&req).expect("rendu réussi");
        assert_eq!(
            kinds(&yaml),
            vec!["Deployment", "Service", "Ingress", "PersistentVolumeClaim"]
        );
    }

    #[test]
    fn deployment_bien_forme() {
        let yaml = render_manifests(&base()).expect("rendu réussi");
        let docs = documents(&yaml);
        let dep = &docs[0];
        assert_eq!(dep["apiVersion"], "apps/v1");
        assert_eq!(dep["metadata"]["name"], "mon-api");
        assert_eq!(dep["metadata"]["namespace"], "production");
        assert_eq!(dep["metadata"]["labels"][LABEL_MANAGED_BY], "kubewatch");
        assert_eq!(dep["metadata"]["labels"][LABEL_NAME], "mon-api");
        assert_eq!(dep["spec"]["replicas"], 2);
        assert_eq!(dep["spec"]["selector"]["matchLabels"][LABEL_NAME], "mon-api");

        let container = &dep["spec"]["template"]["spec"]["containers"][0];
        assert_eq!(container["image"], "ghcr.io/exemple/api:1.4.2");
        assert_eq!(container["ports"][0]["containerPort"], 8080);
        assert_eq!(container["ports"][0]["protocol"], "TCP");
        assert_eq!(container["env"][0]["name"], "LOG_LEVEL");
        assert_eq!(container["securityContext"]["allowPrivilegeEscalation"], false);
        assert_eq!(container["securityContext"]["capabilities"]["drop"][0], "ALL");
        assert_eq!(container["readinessProbe"]["tcpSocket"]["port"], "http");
        assert!(container["livenessProbe"].is_object());
    }

    #[test]
    fn service_reprend_le_port_expose() {
        let yaml = render_manifests(&base()).expect("rendu réussi");
        let service = &documents(&yaml)[1];
        assert_eq!(service["spec"]["type"], "ClusterIP");
        assert_eq!(service["spec"]["ports"][0]["port"], 80);
        assert_eq!(service["spec"]["ports"][0]["targetPort"], "http");
        assert_eq!(service["spec"]["selector"][LABEL_INSTANCE], "mon-api");
    }

    #[test]
    fn ingress_et_volume_bien_relies() {
        let mut req = base();
        req.ingress_host = Some("api.exemple.fr".to_string());
        req.ingress_tls_secret = Some("api-tls".to_string());
        req.pvc = Some(PvcSpec {
            name: Some("api-donnees".to_string()),
            size: "20Gi".to_string(),
            mount_path: "/data".to_string(),
            storage_class: None,
            access_mode: Some("ReadWriteMany".to_string()),
        });
        let yaml = render_manifests(&req).expect("rendu réussi");
        let docs = documents(&yaml);

        let ingress = docs.iter().find(|d| d["kind"] == "Ingress").expect("Ingress généré");
        assert_eq!(ingress["spec"]["rules"][0]["host"], "api.exemple.fr");
        assert_eq!(
            ingress["spec"]["rules"][0]["http"]["paths"][0]["backend"]["service"]["port"]["number"],
            80
        );
        assert_eq!(ingress["spec"]["tls"][0]["secretName"], "api-tls");

        let pvc = docs
            .iter()
            .find(|d| d["kind"] == "PersistentVolumeClaim")
            .expect("PVC généré");
        assert_eq!(pvc["metadata"]["name"], "api-donnees");
        assert_eq!(pvc["spec"]["accessModes"][0], "ReadWriteMany");
        assert_eq!(pvc["spec"]["resources"]["requests"]["storage"], "20Gi");

        let dep = docs.iter().find(|d| d["kind"] == "Deployment").expect("Deployment généré");
        assert_eq!(
            dep["spec"]["template"]["spec"]["volumes"][0]["persistentVolumeClaim"]["claimName"],
            "api-donnees"
        );
        assert_eq!(
            dep["spec"]["template"]["spec"]["containers"][0]["volumeMounts"][0]["mountPath"],
            "/data"
        );
    }

    #[test]
    fn ressources_rendues_quand_elles_sont_fournies() {
        let mut req = base();
        req.cpu_request = Some("100m".to_string());
        req.memory_request = Some("256Mi".to_string());
        req.memory_limit = Some("1Gi".to_string());
        let yaml = render_manifests(&req).expect("rendu réussi");
        let container = &documents(&yaml)[0]["spec"]["template"]["spec"]["containers"][0];
        assert_eq!(container["resources"]["requests"]["cpu"], "100m");
        assert_eq!(container["resources"]["limits"]["memory"], "1Gi");
        assert!(container["resources"]["limits"].get("cpu").is_none());
    }

    #[test]
    fn nom_de_port_genere_quand_il_manque() {
        let mut req = base();
        req.ports[0].name = None;
        let yaml = render_manifests(&req).expect("rendu réussi");
        let container = &documents(&yaml)[0]["spec"]["template"]["spec"]["containers"][0];
        assert_eq!(container["ports"][0]["name"], "p8080");
    }

    #[test]
    fn nom_invalide_rejete() {
        let mut req = base();
        req.name = "Mon_API".to_string();
        let err = validate(&req).expect_err("nom invalide");
        assert!(err.to_string().contains("nom d'application"), "{err}");
    }

    #[test]
    fn namespace_invalide_rejete() {
        let mut req = base();
        req.namespace = "Production!".to_string();
        assert!(validate(&req).is_err());
    }

    #[test]
    fn image_vide_ou_illisible_rejetee() {
        let mut req = base();
        req.image = String::new();
        assert!(validate(&req).is_err());
        req.image = "ghcr.io/".to_string();
        assert!(validate(&req).is_err());
    }

    #[test]
    fn repliques_hors_bornes_rejetees() {
        let mut req = base();
        req.replicas = -1;
        assert!(validate(&req).is_err());
        req.replicas = 1001;
        assert!(validate(&req).is_err());
        req.replicas = 0;
        assert!(validate(&req).is_ok(), "zéro réplique est un arrêt volontaire, pas une erreur");
    }

    #[test]
    fn ports_invalides_rejetes() {
        let mut req = base();
        req.ports[0].container_port = 0;
        assert!(validate(&req).is_err());

        let mut req = base();
        req.ports.push(PortSpec {
            name: Some("http2".to_string()),
            container_port: 8080,
            service_port: Some(81),
            protocol: None,
        });
        assert!(validate(&req).is_err(), "deux fois le même port conteneur");

        let mut req = base();
        req.ports.push(PortSpec {
            name: Some("autre".to_string()),
            container_port: 9090,
            service_port: Some(80),
            protocol: None,
        });
        assert!(validate(&req).is_err(), "deux fois le même port de service");

        let mut req = base();
        req.ports[0].protocol = Some("HTTP".to_string());
        assert!(validate(&req).is_err(), "protocole inconnu");

        let mut req = base();
        req.ports[0].name = Some("un-nom-beaucoup-trop-long".to_string());
        assert!(validate(&req).is_err(), "nom de port trop long");
    }

    #[test]
    fn quantites_invalides_rejetees() {
        let mut req = base();
        req.cpu_request = Some("beaucoup".to_string());
        assert!(validate(&req).is_err());

        let mut req = base();
        req.memory_limit = Some("1Go".to_string());
        assert!(validate(&req).is_err());

        let mut req = base();
        req.cpu_limit = Some("0".to_string());
        assert!(validate(&req).is_err(), "une limite nulle est refusée");
    }

    #[test]
    fn quantites_valides_acceptees() {
        for q in ["100m", "1", "0.5", "2Gi", "500Mi", "1e3", "1.5"] {
            assert!(is_quantity(q), "« {q} » aurait dû être accepté");
        }
        for q in ["", "-1", "abc", "1Go", "1 Gi", "0"] {
            assert!(!is_quantity(q), "« {q} » aurait dû être refusé");
        }
    }

    #[test]
    fn type_de_service_invalide_rejete() {
        let mut req = base();
        req.service_type = Some("Externe".to_string());
        assert!(validate(&req).is_err());
        req.service_type = Some("NodePort".to_string());
        assert!(validate(&req).is_ok());
    }

    #[test]
    fn point_de_montage_relatif_rejete() {
        let mut req = base();
        req.pvc = Some(PvcSpec {
            name: None,
            size: "5Gi".to_string(),
            mount_path: "data".to_string(),
            storage_class: None,
            access_mode: None,
        });
        let err = validate(&req).expect_err("chemin relatif");
        assert!(err.to_string().contains("chemin absolu"), "{err}");

        if let Some(pvc) = req.pvc.as_mut() {
            pvc.mount_path = "/".to_string();
        }
        assert!(validate(&req).is_err(), "la racine est refusée");
    }

    #[test]
    fn ingress_sans_port_rejete() {
        let mut req = base();
        req.ports.clear();
        req.ingress_host = Some("api.exemple.fr".to_string());
        assert!(validate(&req).is_err());
    }

    #[test]
    fn hote_dingress_invalide_rejete() {
        let mut req = base();
        req.ingress_host = Some("api exemple fr".to_string());
        assert!(validate(&req).is_err());
        req.ingress_host = Some("*.exemple.fr".to_string());
        assert!(validate(&req).is_ok(), "un joker de sous-domaine est légitime");
    }

    #[test]
    fn variable_denvironnement_invalide_rejetee() {
        let mut req = base();
        req.env = vec![("MA-VARIABLE".to_string(), "x".to_string())];
        assert!(validate(&req).is_err());
    }

    #[test]
    fn cles_detiquettes_verifiees() {
        assert!(check_metadata_key("app.kubernetes.io/name").is_ok());
        assert!(check_metadata_key("equipe").is_ok());
        assert!(check_metadata_key("").is_err());
        assert!(check_metadata_key("mauvais prefixe/nom").is_err());
        assert!(check_metadata_key("prefixe/").is_err());
    }

    #[test]
    fn requete_par_defaut_deserialisable_depuis_un_json_partiel() {
        let req: DeployRequest = serde_json::from_str(
            r#"{"name":"demo","namespace":"default","image":"nginx:1.27.3-alpine"}"#,
        )
        .expect("désérialisation partielle");
        assert_eq!(req.replicas, 1, "la valeur par défaut doit être une réplique");
        assert!(validate(&req).is_ok());
    }

    #[test]
    fn le_yaml_genere_est_relisible_sans_ambiguite() {
        let mut req = base();
        req.labels.insert("equipe".to_string(), "plateforme".to_string());
        req.annotations
            .insert("kubewatch.io/origine".to_string(), "catalogue".to_string());
        req.node_selector
            .insert("kubernetes.io/os".to_string(), "linux".to_string());
        req.command = vec!["/bin/api".to_string()];
        req.args = vec!["--port".to_string(), "8080".to_string()];
        let yaml = render_manifests(&req).expect("rendu réussi");
        assert!(yaml.starts_with("# Manifestes générés par KubeWatch"));
        let docs = documents(&yaml);
        assert_eq!(docs.len(), 2);
        let dep = &docs[0];
        assert_eq!(dep["metadata"]["labels"]["equipe"], "plateforme");
        assert_eq!(dep["metadata"]["annotations"]["kubewatch.io/origine"], "catalogue");
        assert_eq!(dep["spec"]["template"]["spec"]["nodeSelector"]["kubernetes.io/os"], "linux");
        assert_eq!(dep["spec"]["template"]["spec"]["containers"][0]["command"][0], "/bin/api");
        assert_eq!(dep["spec"]["template"]["spec"]["containers"][0]["args"][1], "8080");
    }
}
