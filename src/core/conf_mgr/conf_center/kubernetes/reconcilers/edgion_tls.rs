//! EdgionTls reconciler

use kube::runtime::controller::Action;
use kube::ResourceExt;
use std::sync::Arc;
use std::time::Duration;

use crate::core::conf_mgr::resource_check::{check_edgion_tls, ResourceCheckContext};
use crate::core::conf_sync::traits::ResourceChange;
use crate::types::prelude_resources::EdgionTls;

use super::super::context::ControllerContext;
use super::super::error::ReconcileError;

/// Reconcile EdgionTls
pub async fn reconcile(
    tls: Arc<EdgionTls>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, ReconcileError> {
    let name = tls.name_any();
    let namespace = tls.namespace().unwrap_or_default();
    let is_deleted = tls.metadata.deletion_timestamp.is_some();

    tracing::debug!(
        component = "k8s_controller",
        kind = "EdgionTls",
        name = %name,
        namespace = %namespace,
        is_deleted = is_deleted,
        "Reconciling EdgionTls"
    );

    if is_deleted {
        ctx.config_server
            .apply_edgion_tls_change(ResourceChange::EventDelete, (*tls).clone());
        return Ok(Action::await_change());
    }

    // Validate before applying
    let check_ctx = ResourceCheckContext::new(&ctx.config_server);
    let check_result = check_edgion_tls(&check_ctx, &tls);
    if check_result.should_skip() {
        tracing::warn!(
            component = "k8s_controller",
            kind = "EdgionTls",
            name = %name,
            namespace = %namespace,
            "EdgionTls validation failed, skipping"
        );
        return Ok(Action::requeue(Duration::from_secs(60)));
    }

    ctx.config_server
        .apply_edgion_tls_change(ResourceChange::EventUpdate, (*tls).clone());
    Ok(Action::requeue(Duration::from_secs(3600)))
}
