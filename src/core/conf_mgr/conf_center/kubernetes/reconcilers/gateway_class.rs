//! GatewayClass reconciler

use kube::runtime::controller::Action;
use kube::ResourceExt;
use std::sync::Arc;
use std::time::Duration;

use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::CacheEventDispatch;
use crate::types::prelude_resources::GatewayClass;

use super::super::context::ControllerContext;
use super::super::error::ReconcileError;

/// Reconcile GatewayClass
pub async fn reconcile(
    class: Arc<GatewayClass>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, ReconcileError> {
    let name = class.name_any();
    let is_deleted = class.metadata.deletion_timestamp.is_some();

    tracing::debug!(
        component = "k8s_controller",
        kind = "GatewayClass",
        name = %name,
        is_deleted = is_deleted,
        "Reconciling GatewayClass"
    );

    if is_deleted {
        ctx.config_server
            .gateway_classes
            .apply_change(ResourceChange::EventDelete, (*class).clone());
        return Ok(Action::await_change());
    }

    ctx.config_server
        .gateway_classes
        .apply_change(ResourceChange::EventUpdate, (*class).clone());
    Ok(Action::requeue(Duration::from_secs(3600)))
}
