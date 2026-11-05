//! Exact source counts use the same resource List authorization as source pages.
use super::bounded_json::BoundedJson as Json;
use super::common::parse_kind;
use super::types::{source_read, AdminState};
use crate::conf_mgr::conf_center::traits::SourceCount;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountQuery {}
#[derive(Serialize)]
pub struct CountResponse {
    success: bool,
    #[serde(flatten)]
    count: SourceCount,
}
async fn count(
    state: Arc<AdminState>,
    kind: &str,
    namespace: Option<&str>,
    cluster: bool,
) -> Result<Json<CountResponse>, StatusCode> {
    let kind = parse_kind(kind).map_err(|_| StatusCode::BAD_REQUEST)?;
    if edgion_resources::resource::is_kind_cluster_scoped(kind) != cluster {
        return Err(StatusCode::BAD_REQUEST);
    }
    let backend = state.conf_center();
    let count = source_read(backend.count_by_kind(kind.as_str(), namespace)).await?;
    Ok(Json(CountResponse { success: true, count }))
}
pub async fn namespaced(
    State(state): State<Arc<AdminState>>,
    Path(kind): Path<String>,
    Query(_): Query<CountQuery>,
) -> Result<Json<CountResponse>, StatusCode> {
    count(state, &kind, None, false).await
}
pub async fn namespace(
    State(state): State<Arc<AdminState>>,
    Path((kind, namespace)): Path<(String, String)>,
    Query(_): Query<CountQuery>,
) -> Result<Json<CountResponse>, StatusCode> {
    count(state, &kind, Some(&namespace), false).await
}
pub async fn cluster(
    State(state): State<Arc<AdminState>>,
    Path(kind): Path<String>,
    Query(_): Query<CountQuery>,
) -> Result<Json<CountResponse>, StatusCode> {
    count(state, &kind, None, true).await
}
