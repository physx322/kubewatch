//! Hub : registres d'images, charts Helm, catalogue et déploiement.

use tauri::State;

use kubewatch_core::apply::{self, ApplyOptions, ApplyOutcome};
use kubewatch_hub::catalog;
use kubewatch_hub::deploy::{self, DeployRequest};
use kubewatch_hub::model::{
    CatalogApp, ChartSummary, ImageDetails, ImageRef, ImageSummary, RegistryKind, TagInfo,
};

use super::fail;
use crate::state::AppState;

const HUB_LIMIT: usize = 60;
const HUB_TAGS_LIMIT: usize = 200;

#[tauri::command]
pub async fn hub_search_images(
    state: State<'_, AppState>,
    query: String,
    registry: RegistryKind,
) -> Result<Vec<ImageSummary>, String> {
    let hub = state.hub()?;
    hub.search_images(&query, registry, HUB_LIMIT)
        .await
        .map_err(|e| fail("recherche d'images impossible", e))
}

#[tauri::command]
pub async fn hub_list_tags(
    state: State<'_, AppState>,
    image: String,
) -> Result<Vec<TagInfo>, String> {
    let hub = state.hub()?;
    let reference = ImageRef::parse(&image)
        .map_err(|e| fail(&format!("référence d'image « {image} » invalide"), e))?;
    hub.list_tags(&reference, HUB_TAGS_LIMIT)
        .await
        .map_err(|e| fail(&format!("tags de « {image} » illisibles"), e))
}

#[tauri::command]
pub async fn hub_inspect(
    state: State<'_, AppState>,
    image: String,
) -> Result<ImageDetails, String> {
    let hub = state.hub()?;
    let reference = ImageRef::parse(&image)
        .map_err(|e| fail(&format!("référence d'image « {image} » invalide"), e))?;
    hub.inspect(&reference)
        .await
        .map_err(|e| fail(&format!("inspection de « {image} » impossible"), e))
}

#[tauri::command]
pub async fn hub_search_charts(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<ChartSummary>, String> {
    let hub = state.hub()?;
    hub.search_charts(&query, HUB_LIMIT)
        .await
        .map_err(|e| fail("recherche de charts impossible", e))
}

#[tauri::command]
pub fn hub_catalog() -> Vec<CatalogApp> {
    catalog::builtin_apps().to_vec()
}

#[tauri::command]
pub fn hub_render(request: DeployRequest) -> Result<String, String> {
    deploy::render_manifests(&request).map_err(|e| fail("génération des manifestes impossible", e))
}

#[tauri::command]
pub async fn hub_deploy(
    state: State<'_, AppState>,
    cluster: String,
    request: DeployRequest,
    dry_run: bool,
) -> Result<ApplyOutcome, String> {
    let h = state.handle(&cluster)?;
    let yaml = deploy::render_manifests(&request)
        .map_err(|e| fail("génération des manifestes impossible", e))?;
    let opts = ApplyOptions {
        dry_run,
        default_namespace: Some(request.namespace.clone()),
        ..Default::default()
    };
    apply::apply_yaml(&h, &yaml, &opts).await.map_err(|e| {
        fail(
            &format!("déploiement de « {} » impossible", request.name),
            e,
        )
    })
}
