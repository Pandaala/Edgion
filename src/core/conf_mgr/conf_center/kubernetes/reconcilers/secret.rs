//! Secret reconciler

use k8s_openapi::api::core::v1::Secret;
use kube::runtime::controller::Action;
use kube::ResourceExt;
use std::sync::Arc;
use std::time::Duration;

use crate::core::conf_sync::traits::ResourceChange;

use super::super::context::ControllerContext;
use super::super::error::ReconcileError;

/// Reconcile Secret
pub async fn reconcile(
    secret: Arc<Secret>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, ReconcileError> {
    let name = secret.name_any();
    let namespace = secret.namespace().unwrap_or_default();
    let is_deleted = secret.metadata.deletion_timestamp.is_some();

    tracing::debug!(
        component = "k8s_controller",
        kind = "Secret",
        name = %name,
        namespace = %namespace,
        is_deleted = is_deleted,
        "Reconciling Secret"
    );

    if is_deleted {
        ctx.config_server
            .apply_secret_change(ResourceChange::EventDelete, (*secret).clone());
        return Ok(Action::await_change());
    }

    ctx.config_server
        .apply_secret_change(ResourceChange::EventUpdate, (*secret).clone());
    Ok(Action::requeue(Duration::from_secs(3600)))
}
