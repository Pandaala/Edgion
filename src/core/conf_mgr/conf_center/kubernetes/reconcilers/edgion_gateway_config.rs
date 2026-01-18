//! EdgionGatewayConfig reconciler

use kube::runtime::controller::Action;
use kube::ResourceExt;
use std::sync::Arc;
use std::time::Duration;

use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::CacheEventDispatch;
use crate::types::prelude_resources::EdgionGatewayConfig;

use super::super::context::ControllerContext;
use super::super::error::ReconcileError;

/// Reconcile EdgionGatewayConfig
pub async fn reconcile(
    config: Arc<EdgionGatewayConfig>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, ReconcileError> {
    let name = config.name_any();
    let is_deleted = config.metadata.deletion_timestamp.is_some();

    tracing::debug!(
        component = "k8s_controller",
        kind = "EdgionGatewayConfig",
        name = %name,
        is_deleted = is_deleted,
        "Reconciling EdgionGatewayConfig"
    );

    if is_deleted {
        ctx.config_server
            .edgion_gateway_configs
            .apply_change(ResourceChange::EventDelete, (*config).clone());
        return Ok(Action::await_change());
    }

    ctx.config_server
        .edgion_gateway_configs
        .apply_change(ResourceChange::EventUpdate, (*config).clone());
    Ok(Action::requeue(Duration::from_secs(3600)))
}
