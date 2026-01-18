//! EdgionStreamPlugins reconciler

use kube::runtime::controller::Action;
use kube::ResourceExt;
use std::sync::Arc;
use std::time::Duration;

use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::CacheEventDispatch;
use crate::types::prelude_resources::EdgionStreamPlugins;

use super::super::context::ControllerContext;
use super::super::error::ReconcileError;

/// Reconcile EdgionStreamPlugins
pub async fn reconcile(
    plugins: Arc<EdgionStreamPlugins>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, ReconcileError> {
    let name = plugins.name_any();
    let namespace = plugins.namespace().unwrap_or_default();
    let is_deleted = plugins.metadata.deletion_timestamp.is_some();

    tracing::debug!(
        component = "k8s_controller",
        kind = "EdgionStreamPlugins",
        name = %name,
        namespace = %namespace,
        is_deleted = is_deleted,
        "Reconciling EdgionStreamPlugins"
    );

    if is_deleted {
        ctx.config_server
            .edgion_stream_plugins
            .apply_change(ResourceChange::EventDelete, (*plugins).clone());
        return Ok(Action::await_change());
    }

    ctx.config_server
        .edgion_stream_plugins
        .apply_change(ResourceChange::EventUpdate, (*plugins).clone());
    Ok(Action::requeue(Duration::from_secs(3600)))
}
