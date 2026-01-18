//! LinkSys reconciler

use kube::runtime::controller::Action;
use kube::ResourceExt;
use std::sync::Arc;
use std::time::Duration;

use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::CacheEventDispatch;
use crate::types::prelude_resources::LinkSys;

use super::super::context::ControllerContext;
use super::super::error::ReconcileError;

/// Reconcile LinkSys
pub async fn reconcile(
    link: Arc<LinkSys>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, ReconcileError> {
    let name = link.name_any();
    let namespace = link.namespace().unwrap_or_default();
    let is_deleted = link.metadata.deletion_timestamp.is_some();

    tracing::debug!(
        component = "k8s_controller",
        kind = "LinkSys",
        name = %name,
        namespace = %namespace,
        is_deleted = is_deleted,
        "Reconciling LinkSys"
    );

    if is_deleted {
        ctx.config_server
            .link_sys
            .apply_change(ResourceChange::EventDelete, (*link).clone());
        return Ok(Action::await_change());
    }

    ctx.config_server
        .link_sys
        .apply_change(ResourceChange::EventUpdate, (*link).clone());
    Ok(Action::requeue(Duration::from_secs(3600)))
}
