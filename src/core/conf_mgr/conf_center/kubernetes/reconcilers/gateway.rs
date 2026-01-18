//! Gateway reconciler

use kube::runtime::controller::Action;
use kube::ResourceExt;
use std::sync::Arc;
use std::time::Duration;

use crate::core::conf_mgr::resource_check::generate_gateway_status;
use crate::core::conf_sync::traits::ResourceChange;
use crate::types::prelude_resources::Gateway;

use super::super::context::ControllerContext;
use super::super::error::ReconcileError;

/// Reconcile Gateway
/// Note: Gateway resources are pre-filtered by gateway_class_name in ResourceController
pub async fn reconcile(
    gateway: Arc<Gateway>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, ReconcileError> {
    let name = gateway.name_any();
    let namespace = gateway.namespace().unwrap_or_default();
    let is_deleted = gateway.metadata.deletion_timestamp.is_some();

    tracing::debug!(
        component = "k8s_controller",
        kind = "Gateway",
        name = %name,
        namespace = %namespace,
        is_deleted = is_deleted,
        "Reconciling Gateway"
    );

    if is_deleted {
        ctx.config_server
            .apply_gateway_change(ResourceChange::EventDelete, (*gateway).clone());
        return Ok(Action::await_change());
    }

    ctx.config_server
        .apply_gateway_change(ResourceChange::EventUpdate, (*gateway).clone());

    // Update K8s Status
    if let Some(status) = generate_gateway_status(&gateway, &ctx.gateway_class_name) {
        if let Err(e) = ctx
            .status_store
            .update_gateway_status(&namespace, &name, status)
            .await
        {
            tracing::warn!(error = %e, "Failed to update Gateway status");
        }
    }

    Ok(Action::requeue(Duration::from_secs(3600)))
}
