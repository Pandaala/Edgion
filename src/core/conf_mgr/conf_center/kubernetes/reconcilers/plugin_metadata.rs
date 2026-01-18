//! PluginMetaData reconciler

use kube::runtime::controller::Action;
use kube::ResourceExt;
use std::sync::Arc;
use std::time::Duration;

use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::CacheEventDispatch;
use crate::types::prelude_resources::PluginMetaData;

use super::super::context::ControllerContext;
use super::super::error::ReconcileError;

/// Reconcile PluginMetaData
pub async fn reconcile(
    metadata: Arc<PluginMetaData>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, ReconcileError> {
    let name = metadata.name_any();
    let namespace = metadata.namespace().unwrap_or_default();
    let is_deleted = metadata.metadata.deletion_timestamp.is_some();

    tracing::debug!(
        component = "k8s_controller",
        kind = "PluginMetaData",
        name = %name,
        namespace = %namespace,
        is_deleted = is_deleted,
        "Reconciling PluginMetaData"
    );

    if is_deleted {
        ctx.config_server
            .plugin_metadata
            .apply_change(ResourceChange::EventDelete, (*metadata).clone());
        return Ok(Action::await_change());
    }

    ctx.config_server
        .plugin_metadata
        .apply_change(ResourceChange::EventUpdate, (*metadata).clone());
    Ok(Action::requeue(Duration::from_secs(3600)))
}
