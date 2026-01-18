//! EndpointSlice reconciler

use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::runtime::controller::Action;
use kube::ResourceExt;
use std::sync::Arc;
use std::time::Duration;

use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::CacheEventDispatch;

use super::super::context::ControllerContext;
use super::super::error::ReconcileError;

/// Reconcile EndpointSlice
pub async fn reconcile(
    slice: Arc<EndpointSlice>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, ReconcileError> {
    let name = slice.name_any();
    let namespace = slice.namespace().unwrap_or_default();
    let is_deleted = slice.metadata.deletion_timestamp.is_some();

    tracing::debug!(
        component = "k8s_controller",
        kind = "EndpointSlice",
        name = %name,
        namespace = %namespace,
        is_deleted = is_deleted,
        "Reconciling EndpointSlice"
    );

    if is_deleted {
        ctx.config_server
            .endpoint_slices
            .apply_change(ResourceChange::EventDelete, (*slice).clone());
        return Ok(Action::await_change());
    }

    ctx.config_server
        .endpoint_slices
        .apply_change(ResourceChange::EventUpdate, (*slice).clone());
    Ok(Action::requeue(Duration::from_secs(3600)))
}
