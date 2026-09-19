//! Charts Helm : recherche sur Artifact Hub et rendu local via le binaire `helm`.
//!
//! KubeWatch ne réimplémente pas le moteur de templates Go de Helm : rendre un chart exige
//! le binaire officiel. En son absence, [`render_chart`] renvoie une erreur explicite qui
//! oriente vers le générateur de manifestes intégré ([`crate::deploy::render_manifests`]),
//! lequel couvre les déploiements simples sans aucune dépendance externe.

use crate::error::{Error, Result};
use crate::model::ChartSummary;
use crate::registry::HubClient;
use std::path::PathBuf;

/// Racine de l'API publique d'Artifact Hub.
const ARTIFACT_HUB_API: &str = "https://artifacthub.io/api/v1";

/// Préfixe des images d'icônes servies par Artifact Hub.
const ARTIFACT_HUB_IMAGE: &str = "https://artifacthub.io/image";

/// Variable d'environnement permettant de forcer le chemin du binaire `helm`.
const HELM_BIN_ENV: &str = "KUBEWATCH_HELM_BIN";

/// Durée maximale accordée au rendu d'un chart.
const RENDER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

impl HubClient {
    /// Recherche des charts Helm sur Artifact Hub.
    pub async fn search_charts(&self, q: &str, limit: usize) -> Result<Vec<ChartSummary>> {
        let q = q.trim();
        if q.is_empty() {
            return Err(Error::Invalid(
                "recherche vide : saisissez un nom de chart, par exemple « postgresql »"
                    .to_string(),
            ));
        }
        let limit = limit.clamp(1, 60);
        let url = format!("{ARTIFACT_HUB_API}/packages/search");
        let body = self
            .get_json(
                &url,
                &[
                    // kind=0 : charts Helm uniquement.
                    ("kind", "0".to_string()),
                    ("ts_query_web", q.to_string()),
                    ("limit", limit.to_string()),
                    ("facets", "false".to_string()),
                    ("sort", "relevance".to_string()),
                ],
                "recherche de charts Helm",
            )
            .await?;

        let packages = body
            .get("packages")
            .and_then(|v| v.as_array())
            .cloned()
            .or_else(|| body.as_array().cloned())
            .unwrap_or_default();

        Ok(packages.iter().filter_map(chart_from_package).take(limit).collect())
    }

    /// Liste les versions publiées d'un chart, de la plus récente à la plus ancienne.
    pub async fn chart_versions(&self, repo: &str, name: &str) -> Result<Vec<ChartSummary>> {
        let package = self.chart_package(repo, name, None).await?;
        let base = chart_from_package(&package).ok_or_else(|| {
            Error::NotFound(format!("chart « {repo}/{name} » introuvable sur Artifact Hub"))
        })?;

        let mut out: Vec<ChartSummary> = Vec::new();
        for entry in package
            .get("available_versions")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            let Some(version) = entry.get("version").and_then(|v| v.as_str()) else { continue };
            let mut summary = base.clone();
            summary.version = version.to_string();
            summary.app_version = entry
                .get("app_version")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| base.app_version.clone());
            out.push(summary);
        }

        if out.is_empty() {
            out.push(base);
        }
        // Artifact Hub ne garantit pas l'ordre : on trie en version sémantique décroissante.
        out.sort_by(|a, b| {
            match (
                semver::Version::parse(&a.version),
                semver::Version::parse(&b.version),
            ) {
                (Ok(x), Ok(y)) => y.cmp(&x),
                (Ok(_), Err(_)) => std::cmp::Ordering::Less,
                (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
                (Err(_), Err(_)) => b.version.cmp(&a.version),
            }
        });
        Ok(out)
    }

    /// Récupère le fichier `values.yaml` par défaut d'une version de chart.
    pub async fn chart_values(&self, repo: &str, name: &str, version: &str) -> Result<String> {
        let (repo, name) = check_chart_coordinates(repo, name)?;
        let version = version.trim();
        if version.is_empty() {
            return Err(Error::Invalid(
                "version de chart manquante : indiquez par exemple « 15.5.1 »".to_string(),
            ));
        }
        let url = format!(
            "{ARTIFACT_HUB_API}/packages/helm/{}/{}/{}/values",
            urlencoding::encode(&repo),
            urlencoding::encode(&name),
            urlencoding::encode(version)
        );
        let values = self
            .get_text(&url, &[], &format!("valeurs du chart « {repo}/{name} » {version}"))
            .await?;
        Ok(values)
    }

    /// Lit la fiche d'un chart sur Artifact Hub, éventuellement à une version donnée.
    async fn chart_package(
        &self,
        repo: &str,
        name: &str,
        version: Option<&str>,
    ) -> Result<serde_json::Value> {
        let (repo, name) = check_chart_coordinates(repo, name)?;
        let mut url = format!(
            "{ARTIFACT_HUB_API}/packages/helm/{}/{}",
            urlencoding::encode(&repo),
            urlencoding::encode(&name)
        );
        if let Some(v) = version.map(str::trim).filter(|v| !v.is_empty()) {
            url.push('/');
            url.push_str(&urlencoding::encode(v));
        }
        self.get_json(&url, &[], &format!("fiche du chart « {repo}/{name} »"))
            .await
    }
}

/// Vérifie et normalise les coordonnées `dépôt/chart` d'Artifact Hub.
fn check_chart_coordinates(repo: &str, name: &str) -> Result<(String, String)> {
    let repo = repo.trim();
    let name = name.trim();
    if repo.is_empty() || name.is_empty() {
        return Err(Error::Invalid(
            "coordonnées de chart incomplètes : indiquez le dépôt et le nom, par exemple \
             « bitnami/postgresql »"
                .to_string(),
        ));
    }
    if repo.contains('/') || name.contains('/') {
        return Err(Error::Invalid(format!(
            "coordonnées de chart invalides : « {repo}/{name} » ne doit comporter qu'un dépôt et \
             qu'un nom de chart"
        )));
    }
    Ok((repo.to_string(), name.to_string()))
}

/// Convertit une fiche Artifact Hub en [`ChartSummary`].
fn chart_from_package(package: &serde_json::Value) -> Option<ChartSummary> {
    let name = package
        .get("normalized_name")
        .or_else(|| package.get("name"))
        .and_then(|v| v.as_str())?
        .to_string();
    let repository = package
        .get("repository")
        .and_then(|r| r.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if repository.is_empty() {
        return None;
    }
    let icon = package
        .get("logo_image_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|id| format!("{ARTIFACT_HUB_IMAGE}/{id}"));

    Some(ChartSummary {
        name,
        repository,
        version: package
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        app_version: package
            .get("app_version")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        description: package
            .get("description")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        icon,
        stars: package.get("stars").and_then(|v| v.as_i64()),
        home: package
            .get("home_url")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        repo_url: package
            .get("repository")
            .and_then(|r| r.get("url"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
    })
}

/// Localise le binaire `helm`, en honorant d'abord la variable `KUBEWATCH_HELM_BIN`.
pub fn helm_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var(HELM_BIN_ENV) {
        let path = PathBuf::from(path.trim());
        if path.is_file() {
            return Some(path);
        }
    }
    which::which("helm").ok()
}

/// Message d'aide affiché quand `helm` est introuvable.
fn helm_absent() -> Error {
    Error::Unsupported(
        "le binaire « helm » est introuvable sur cette machine : KubeWatch délègue le rendu des \
         charts au client Helm officiel plutôt que de réimplémenter son moteur de gabarits.\n\
         Installation : « brew install helm » (macOS), « winget install Helm.Helm » (Windows), \
         « sudo snap install helm --classic » ou le script officiel \
         « curl -fsSL https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | bash » \
         (Linux). Vous pouvez aussi désigner un binaire existant avec la variable \
         d'environnement KUBEWATCH_HELM_BIN.\n\
         Alternative immédiate : déployez l'image directement via le générateur de manifestes \
         de KubeWatch (onglet « Déployer »), qui produit Deployment, Service, Ingress et PVC \
         sans dépendance externe."
            .to_string(),
    )
}

/// Rend un chart Helm en manifestes YAML, sans contacter le cluster.
///
/// Exécute `helm template <release> <chart> --repo <repo_url> [--version …] --namespace <ns>
/// [-f valeurs]`. Le résultat est un document multi-YAML directement applicable via
/// `kubewatch-core::apply`.
pub async fn render_chart(
    repo_url: &str,
    chart: &str,
    version: Option<&str>,
    release: &str,
    namespace: &str,
    values_yaml: Option<&str>,
) -> Result<String> {
    let helm = helm_binary().ok_or_else(helm_absent)?;

    let repo_url = repo_url.trim();
    let chart = chart.trim();
    let release = release.trim();
    let namespace = namespace.trim();

    if repo_url.is_empty() {
        return Err(Error::Invalid(
            "URL du dépôt Helm manquante : indiquez par exemple \
             « https://charts.bitnami.com/bitnami »"
                .to_string(),
        ));
    }
    if !repo_url.starts_with("http://")
        && !repo_url.starts_with("https://")
        && !repo_url.starts_with("oci://")
    {
        return Err(Error::Invalid(format!(
            "URL du dépôt Helm « {repo_url} » invalide : elle doit commencer par http://, \
             https:// ou oci://"
        )));
    }
    if chart.is_empty() || chart.contains('/') {
        return Err(Error::Invalid(format!(
            "nom de chart « {chart} » invalide : indiquez le nom seul, le dépôt étant passé \
             séparément"
        )));
    }
    if !crate::deploy::is_dns1123_label(release) {
        return Err(Error::Invalid(format!(
            "nom de release « {release} » invalide : il doit être en minuscules, composé de \
             lettres, chiffres et tirets, et commencer et finir par un caractère alphanumérique"
        )));
    }
    if !crate::deploy::is_dns1123_label(namespace) {
        return Err(Error::Invalid(format!(
            "namespace « {namespace} » invalide : il doit respecter le format DNS-1123"
        )));
    }

    let mut command = tokio::process::Command::new(&helm);
    command
        .arg("template")
        .arg(release)
        .arg(chart)
        .arg("--repo")
        .arg(repo_url)
        .arg("--namespace")
        .arg(namespace)
        .arg("--include-crds")
        // Le rendu doit rester hors-ligne vis-à-vis du cluster : aucune API n'est interrogée.
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null());

    if let Some(v) = version.map(str::trim).filter(|v| !v.is_empty()) {
        command.arg("--version").arg(v);
    }

    // Le fichier temporaire doit vivre jusqu'à la fin du processus enfant.
    let values_file = match values_yaml.map(str::trim).filter(|v| !v.is_empty()) {
        Some(values) => {
            let mut file = tempfile::Builder::new()
                .prefix("kubewatch-values-")
                .suffix(".yaml")
                .tempfile()?;
            use std::io::Write;
            file.write_all(values.as_bytes())?;
            file.flush()?;
            command.arg("--values").arg(file.path());
            Some(file)
        }
        None => None,
    };

    let output = tokio::time::timeout(RENDER_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            Error::Other(format!(
                "le rendu du chart « {chart} » a dépassé {} s",
                RENDER_TIMEOUT.as_secs()
            ))
        })??;
    drop(values_file);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        let detail = if detail.is_empty() { "aucun détail fourni par helm" } else { detail };
        return Err(Error::Other(format!(
            "helm n'a pas pu rendre le chart « {chart} » depuis « {repo_url} » : {detail}"
        )));
    }

    let rendered = String::from_utf8(output.stdout).map_err(|_| {
        Error::Other("helm a produit une sortie qui n'est pas de l'UTF-8 valide".to_string())
    })?;
    if rendered.trim().is_empty() {
        return Err(Error::Other(format!(
            "le chart « {chart} » n'a produit aucun manifeste : vérifiez les valeurs fournies"
        )));
    }
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordonnees_de_chart_validees() {
        assert!(check_chart_coordinates("bitnami", "postgresql").is_ok());
        assert!(check_chart_coordinates("", "postgresql").is_err());
        assert!(check_chart_coordinates("bitnami", "").is_err());
        assert!(check_chart_coordinates("bitnami/x", "postgresql").is_err());
    }

    #[test]
    fn conversion_dune_fiche_artifact_hub() {
        let package = serde_json::json!({
            "name": "PostgreSQL",
            "normalized_name": "postgresql",
            "version": "15.5.1",
            "app_version": "16.4.0",
            "description": "Base de données relationnelle",
            "logo_image_id": "abc123",
            "stars": 42,
            "home_url": "https://www.postgresql.org",
            "repository": {"name": "bitnami", "url": "https://charts.bitnami.com/bitnami"}
        });
        let c = chart_from_package(&package).expect("fiche convertible");
        assert_eq!(c.name, "postgresql");
        assert_eq!(c.repository, "bitnami");
        assert_eq!(c.version, "15.5.1");
        assert_eq!(c.app_version.as_deref(), Some("16.4.0"));
        assert_eq!(c.icon.as_deref(), Some("https://artifacthub.io/image/abc123"));
        assert_eq!(c.repo_url.as_deref(), Some("https://charts.bitnami.com/bitnami"));
        assert_eq!(c.stars, Some(42));
    }

    #[test]
    fn fiche_sans_depot_ignoree() {
        let package = serde_json::json!({"name": "orphelin", "version": "1.0.0"});
        assert!(chart_from_package(&package).is_none());
    }

    #[tokio::test]
    async fn parametres_de_rendu_rejetes_avant_execution() {
        // Ces vérifications précèdent tout appel à helm, mais seulement si helm existe :
        // sans binaire, l'erreur attendue est explicitement « non pris en charge ».
        let err = render_chart("ftp://mauvais", "postgresql", None, "db", "default", None)
            .await
            .expect_err("URL invalide");
        assert!(
            matches!(err, Error::Invalid(_) | Error::Unsupported(_)),
            "erreur inattendue : {err}"
        );

        let err = render_chart(
            "https://charts.bitnami.com/bitnami",
            "postgresql",
            None,
            "Nom_Invalide",
            "default",
            None,
        )
        .await
        .expect_err("nom de release invalide");
        assert!(
            matches!(err, Error::Invalid(_) | Error::Unsupported(_)),
            "erreur inattendue : {err}"
        );
    }

    #[test]
    fn message_dabsence_de_helm_actionnable() {
        let msg = helm_absent().to_string();
        assert!(msg.contains("helm"));
        assert!(msg.contains("KUBEWATCH_HELM_BIN"));
        assert!(msg.contains("générateur de manifestes"));
    }
}
