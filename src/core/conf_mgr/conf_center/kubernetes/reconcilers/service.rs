//! Service reconciler

use k8s_openapi::api::core::v1::Service;
use kube::runtime::controller::Action;
use kube::ResourceExt;
use std::sync::Arc;
use std::time::Duration;

use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::CacheEventDispatch;

use super::super::context::ControllerContext;
use super::super::error::ReconcileError;

/// Reconcile Service
pub async fn reconcile(
    service: Arc<Service>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, ReconcileError> {
    let name = service.name_any();
    let namespace = service.namespace().unwrap_or_default();
    let is_deleted = service.metadata.deletion_timestamp.is_some();

    tracing::debug!(
        component = "k8s_controller",
        kind = "Service",
        name = %name,
        namespace = %namespace,
        is_deleted = is_deleted,
        "Reconciling Service"
    );

    if is_deleted {
        ctx.config_server
            .services
            .apply_change(ResourceChange::EventDelete, (*service).clone());
        return Ok(Action::await_change());
    }

    ctx.config_server
        .services
        .apply_change(ResourceChange::EventUpdate, (*service).clone());
    Ok(Action::requeue(Duration::from_secs(3600)))
}
