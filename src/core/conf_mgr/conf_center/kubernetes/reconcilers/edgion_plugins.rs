//! EdgionPlugins reconciler

use kube::runtime::controller::Action;
use kube::ResourceExt;
use std::sync::Arc;
use std::time::Duration;

use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::CacheEventDispatch;
use crate::types::prelude_resources::EdgionPlugins;

use super::super::context::ControllerContext;
use super::super::error::ReconcileError;

/// Reconcile EdgionPlugins
pub async fn reconcile(
    plugins: Arc<EdgionPlugins>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, ReconcileError> {
    let name = plugins.name_any();
    let namespace = plugins.namespace().unwrap_or_default();
    let is_deleted = plugins.metadata.deletion_timestamp.is_some();

    tracing::debug!(
        component = "k8s_controller",
        kind = "EdgionPlugins",
        name = %name,
        namespace = %namespace,
        is_deleted = is_deleted,
        "Reconciling EdgionPlugins"
    );

    if is_deleted {
        ctx.config_server
            .edgion_plugins
            .apply_change(ResourceChange::EventDelete, (*plugins).clone());
        return Ok(Action::await_change());
    }

    ctx.config_server
        .edgion_plugins
        .apply_change(ResourceChange::EventUpdate, (*plugins).clone());
    Ok(Action::requeue(Duration::from_secs(3600)))
}
