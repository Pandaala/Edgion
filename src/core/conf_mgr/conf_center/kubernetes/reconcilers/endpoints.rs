//! Endpoints reconciler

use k8s_openapi::api::core::v1::Endpoints;
use kube::runtime::controller::Action;
use kube::ResourceExt;
use std::sync::Arc;
use std::time::Duration;

use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::CacheEventDispatch;

use super::super::context::ControllerContext;
use super::super::error::ReconcileError;

/// Reconcile Endpoints
pub async fn reconcile(
    endpoints: Arc<Endpoints>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, ReconcileError> {
    let name = endpoints.name_any();
    let namespace = endpoints.namespace().unwrap_or_default();
    let is_deleted = endpoints.metadata.deletion_timestamp.is_some();

    tracing::debug!(
        component = "k8s_controller",
        kind = "Endpoints",
        name = %name,
        namespace = %namespace,
        is_deleted = is_deleted,
        "Reconciling Endpoints"
    );

    if is_deleted {
        ctx.config_server
            .endpoints
            .apply_change(ResourceChange::EventDelete, (*endpoints).clone());
        return Ok(Action::await_change());
    }

    ctx.config_server
        .endpoints
        .apply_change(ResourceChange::EventUpdate, (*endpoints).clone());
    Ok(Action::requeue(Duration::from_secs(3600)))
}
