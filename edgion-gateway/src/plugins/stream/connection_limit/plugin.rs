//! L4 ConnectionLimit stream plugin.
//!
//! Stateless: the attachment scope owner (listener filter, TCP proxy, or UDP
//! proxy) counts the connection before the chain runs and passes its ordinal in
//! [`StreamContext::active_connections`]. The plugin denies when the ordinal is
//! above `maxConnections`, so the cap is never exceeded and a config reload
//! cannot reset the count. NO tracing on this data path; a denial records the
//! bounded rejection metric.

use async_trait::async_trait;

use crate::observe::metrics::{global_metrics, DownstreamConnectionRejectReason};
use crate::plugins::stream::stream_plugin_trait::{StreamContext, StreamLayer, StreamPlugin, StreamPluginResult};
use edgion_resources::resources::edgion_plugins::PluginValidation;
use edgion_resources::resources::edgion_stream_plugins::ConnectionLimitConfig;

const PLUGIN_NAME: &str = "ConnectionLimit";

pub struct ConnectionLimitPlugin {
    max_connections: u64,
}

impl ConnectionLimitPlugin {
    /// Validate and build. An invalid config is an error so the runtime keeps
    /// its fail-closed construction path.
    pub fn new(config: &ConnectionLimitConfig) -> Result<Self, &'static str> {
        if config.validation_error().is_some() {
            return Err("invalid stream/ConnectionLimit configuration");
        }
        Ok(Self {
            max_connections: config.max_connections,
        })
    }
}

#[async_trait]
impl StreamPlugin for ConnectionLimitPlugin {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn on_connection(&self, ctx: &StreamContext) -> StreamPluginResult {
        if ctx.active_connections <= self.max_connections {
            return StreamPluginResult::Allow;
        }
        global_metrics().downstream_connection_rejected(reject_reason(ctx.layer));
        StreamPluginResult::Deny("connection-limit".into())
    }
}

/// Metric reason for a denial at `layer`: Stage-1 runs on the Gateway
/// listener; the TCP layer is the TCPRoute/UDPRoute route stage.
fn reject_reason(layer: StreamLayer) -> DownstreamConnectionRejectReason {
    match layer {
        StreamLayer::Stage1 => DownstreamConnectionRejectReason::Listener,
        StreamLayer::Tcp => DownstreamConnectionRejectReason::Route,
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/stream/connection_limit/plugin_test.rs"]
mod tests;
