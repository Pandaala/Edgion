//! ReferenceGrant reconciler

use kube::runtime::controller::Action;
use kube::ResourceExt;
use std::sync::Arc;
use std::time::Duration;

use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::CacheEventDispatch;
use crate::types::prelude_resources::ReferenceGrant;

use super::super::context::ControllerContext;
use super::super::error::ReconcileError;

/// Reconcile ReferenceGrant
pub async fn reconcile(
    grant: Arc<ReferenceGrant>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, ReconcileError> {
    let name = grant.name_any();
    let namespace = grant.namespace().unwrap_or_default();
    let is_deleted = grant.metadata.deletion_timestamp.is_some();

    tracing::debug!(
        component = "k8s_controller",
        kind = "ReferenceGrant",
        name = %name,
        namespace = %namespace,
        is_deleted = is_deleted,
        "Reconciling ReferenceGrant"
    );

    if is_deleted {
        ctx.config_server
            .reference_grants
            .apply_change(ResourceChange::EventDelete, (*grant).clone());
        return Ok(Action::await_change());
    }

    ctx.config_server
        .reference_grants
        .apply_change(ResourceChange::EventUpdate, (*grant).clone());
    Ok(Action::requeue(Duration::from_secs(3600)))
}
