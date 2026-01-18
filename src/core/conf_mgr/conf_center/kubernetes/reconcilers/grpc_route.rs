//! GRPCRoute reconciler

use kube::runtime::controller::Action;
use kube::ResourceExt;
use std::sync::Arc;
use std::time::Duration;

use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::CacheEventDispatch;
use crate::types::prelude_resources::GRPCRoute;

use super::super::context::ControllerContext;
use super::super::error::ReconcileError;

/// Reconcile GRPCRoute
pub async fn reconcile(
    route: Arc<GRPCRoute>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, ReconcileError> {
    let name = route.name_any();
    let namespace = route.namespace().unwrap_or_default();
    let is_deleted = route.metadata.deletion_timestamp.is_some();

    tracing::debug!(
        component = "k8s_controller",
        kind = "GRPCRoute",
        name = %name,
        namespace = %namespace,
        is_deleted = is_deleted,
        "Reconciling GRPCRoute"
    );

    if is_deleted {
        ctx.config_server
            .grpc_routes
            .apply_change(ResourceChange::EventDelete, (*route).clone());
        return Ok(Action::await_change());
    }

    ctx.config_server
        .grpc_routes
        .apply_change(ResourceChange::EventUpdate, (*route).clone());
    Ok(Action::requeue(Duration::from_secs(3600)))
}
