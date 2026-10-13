//! Physical-peer connection ACL over referenced central IpLists.
use crate::plugins::common::ip_acl::CompiledIpAcl;
use crate::plugins::runtime::edgion_config_data::capture_global_edgion_config_data_snapshot;
use crate::plugins::stream::stream_plugin_trait::{
    access_decision_to_stream_result, StreamContext, StreamPlugin, StreamPluginResult,
};
use async_trait::async_trait;
use edgion_resources::refs::ip_list_fields::ConnectionIpListEntry;
use edgion_resources::refs::ReferenceError;
use edgion_resources::resources::edgion_plugins::PluginValidation;
use std::sync::Arc;
const PLUGIN_NAME: &str = "StreamIpRestriction";
pub struct StreamIpRestriction {
    acl: Arc<CompiledIpAcl>,
}
impl StreamIpRestriction {
    pub(crate) fn from_bound(entry: ConnectionIpListEntry<'_>) -> Result<Self, ReferenceError> {
        if entry.config().validation_error().is_some() {
            return Err(ReferenceError::UnsupportedTarget);
        }
        Ok(Self {
            acl: CompiledIpAcl::from_bound(entry.references(), &entry.config().default_action)?,
        })
    }
}
#[async_trait]
impl StreamPlugin for StreamIpRestriction {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }
    async fn on_connection(&self, ctx: &StreamContext) -> StreamPluginResult {
        let view = capture_global_edgion_config_data_snapshot();
        access_decision_to_stream_result(self.acl.evaluate_in(Some(&view), Some(ctx.direct_peer_ip)), || {})
    }
}
#[cfg(test)]
#[path = "../../../../tests/unit/plugins/stream/ip_restriction/stream_ip_restriction_test.rs"]
mod tests;
