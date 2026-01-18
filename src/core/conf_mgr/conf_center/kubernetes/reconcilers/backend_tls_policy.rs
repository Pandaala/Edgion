//! BackendTLSPolicy reconciler

use kube::runtime::controller::Action;
use kube::ResourceExt;
use std::sync::Arc;
use std::time::Duration;

use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::CacheEventDispatch;
use crate::types::prelude_resources::BackendTLSPolicy;

use super::super::context::ControllerContext;
use super::super::error::ReconcileError;

/// Reconcile BackendTLSPolicy
pub async fn reconcile(
    policy: Arc<BackendTLSPolicy>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, ReconcileError> {
    let name = policy.name_any();
    let namespace = policy.namespace().unwrap_or_default();
    let is_deleted = policy.metadata.deletion_timestamp.is_some();

    tracing::debug!(
        component = "k8s_controller",
        kind = "BackendTLSPolicy",
        name = %name,
        namespace = %namespace,
        is_deleted = is_deleted,
        "Reconciling BackendTLSPolicy"
    );

    if is_deleted {
        ctx.config_server
            .backend_tls_policies
            .apply_change(ResourceChange::EventDelete, (*policy).clone());
        return Ok(Action::await_change());
    }

    ctx.config_server
        .backend_tls_policies
        .apply_change(ResourceChange::EventUpdate, (*policy).clone());
    Ok(Action::requeue(Duration::from_secs(3600)))
}
