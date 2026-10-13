//! TLS-route ACL evaluated after route matching from the selected peer IP source.
use super::tls_route_plugin_trait::{TlsRouteContext, TlsRoutePlugin};
use crate::plugins::common::ip_acl::CompiledIpAcl;
use crate::plugins::runtime::edgion_config_data::capture_global_edgion_config_data_snapshot;
use crate::plugins::stream::stream_plugin_trait::access_decision_to_stream_result;
use crate::plugins::stream::StreamPluginResult;
use async_trait::async_trait;
use edgion_resources::refs::ip_list_fields::HttpIpListEntry;
use edgion_resources::refs::ReferenceError;
use edgion_resources::resources::edgion_plugins::{IpSource, PluginValidation};
use std::sync::Arc;
pub struct TlsRouteIpRestriction {
    ip_source: IpSource,
    acl: Arc<CompiledIpAcl>,
}
impl TlsRouteIpRestriction {
    pub(crate) fn from_bound(entry: HttpIpListEntry<'_>) -> Result<Self, ReferenceError> {
        if entry.config().validation_error().is_some() {
            return Err(ReferenceError::UnsupportedTarget);
        }
        Ok(Self {
            ip_source: entry.config().ip_source.clone(),
            acl: CompiledIpAcl::from_bound(entry.references(), &entry.config().default_action)?,
        })
    }
}
#[async_trait]
impl TlsRoutePlugin for TlsRouteIpRestriction {
    async fn on_tls_route(&self, ctx: &TlsRouteContext<'_>) -> StreamPluginResult {
        let ip = match self.ip_source {
            IpSource::RemoteIp => ctx.remote_ip,
            IpSource::DirectPeerIp => ctx.direct_peer_ip,
        };
        let view = capture_global_edgion_config_data_snapshot();
        access_decision_to_stream_result(self.acl.evaluate_in(Some(&view), Some(ip)), || {})
    }
}
#[cfg(test)]
#[path = "../../../../tests/unit/plugins/stream/tls_route/ip_restriction_test.rs"]
mod tests;
