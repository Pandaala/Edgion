//! HTTPRoute reconciler

use kube::runtime::controller::Action;
use kube::ResourceExt;
use std::sync::Arc;
use std::time::Duration;

use crate::core::conf_mgr::resource_check::generate_http_route_status;
use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::CacheEventDispatch;
use crate::types::prelude_resources::HTTPRoute;

use super::super::context::ControllerContext;
use super::super::error::ReconcileError;

/// Reconcile HTTPRoute
/// Only handles EventUpdate/EventDelete (initial sync done via reflector store)
pub async fn reconcile(
    route: Arc<HTTPRoute>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, ReconcileError> {
    let name = route.name_any();
    let namespace = route.namespace().unwrap_or_default();
    let is_deleted = route.metadata.deletion_timestamp.is_some();

    tracing::debug!(
        component = "k8s_controller",
        kind = "HTTPRoute",
        name = %name,
        namespace = %namespace,
        is_deleted = is_deleted,
        "Reconciling HTTPRoute"
    );

    if is_deleted {
        ctx.config_server
            .routes
            .apply_change(ResourceChange::EventDelete, (*route).clone());
        return Ok(Action::await_change());
    }

    // Runtime event - always EventUpdate
    ctx.config_server
        .routes
        .apply_change(ResourceChange::EventUpdate, (*route).clone());

    // Update K8s Status
    if let Some(status) = generate_http_route_status(&route) {
        if let Err(e) = ctx
            .status_store
            .update_http_route_status(&namespace, &name, status)
            .await
        {
            tracing::warn!(error = %e, "Failed to update HTTPRoute status");
        }
    }

    Ok(Action::requeue(Duration::from_secs(3600)))
}
