//! Stream plugin trait definition

use async_trait::async_trait;
use std::net::IpAddr;

use edgion_resources::resources::edgion_plugins::{AccessDecision, DefaultAction};

/// Result of stream plugin execution
#[derive(Debug, Clone)]
pub enum StreamPluginResult {
    /// Allow the connection to proceed
    Allow,
    /// Deny the connection with a reason
    Deny(String),
}

/// Execution layer for a stream plugin chain.
///
/// This is intentionally closed because it is emitted as the bounded `layer`
/// label on `edgion_geoip_denied_total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamLayer {
    /// Listener-level connection filter, before route matching.
    Stage1,
    /// TCPRoute-level plugin execution.
    Tcp,
}

impl StreamLayer {
    /// Return the bounded metric-label representation.
    pub(crate) const fn as_metric_label(self) -> &'static str {
        match self {
            Self::Stage1 => "stage1",
            Self::Tcp => "tcp",
        }
    }
}

/// Translate an [`AccessDecision`] into a [`StreamPluginResult`], invoking
/// `on_deny` once on each deny outcome (used by the connection-IP-restriction
/// plugins that also need to record a deny metric).
///
/// Centralised here so the deny-reason string literals (`ip:deny:group=...`,
/// `ip:deny:not-in-allow`, `ip:deny:default`) cannot drift between the three
/// stream IP-restriction call sites — any future format change applies in one
/// place and downstream consumers parsing deny reasons (metrics labels, audit
/// logs) stay in sync.
pub(crate) fn access_decision_to_stream_result(decision: AccessDecision, on_deny: impl FnOnce()) -> StreamPluginResult {
    match decision {
        AccessDecision::AllowByGroup(_) => StreamPluginResult::Allow,
        AccessDecision::DenyByGroup(g) => {
            on_deny();
            StreamPluginResult::Deny(format!("ip:deny:group={}", g))
        }
        AccessDecision::DenyByWhitelistMiss => {
            on_deny();
            StreamPluginResult::Deny("ip:deny:not-in-allow".to_string())
        }
        AccessDecision::Default(DefaultAction::Allow) => StreamPluginResult::Allow,
        AccessDecision::Default(DefaultAction::Deny) => {
            on_deny();
            StreamPluginResult::Deny("ip:deny:default".to_string())
        }
    }
}

/// Context for stream plugin execution
#[derive(Debug, Clone)]
pub struct StreamContext {
    /// Physical downstream transport peer IP.
    pub direct_peer_ip: IpAddr,
    /// Listener port that received the connection
    pub listener_port: u16,
    /// Explicit execution layer used by bounded per-layer metrics.
    pub layer: StreamLayer,
}

impl StreamContext {
    /// Create a new stream context for an explicit execution layer.
    pub fn new(direct_peer_ip: IpAddr, listener_port: u16, layer: StreamLayer) -> Self {
        Self {
            direct_peer_ip,
            listener_port,
            layer,
        }
    }
}

/// Connection-context plugin contract used at listener admission, during
/// TCP/UDP route-owned lifecycles. TLSRoute configurations use the separate
/// [`crate::plugins::stream::TlsRoutePlugin`] contract.
#[async_trait]
pub trait StreamPlugin: Send + Sync {
    /// Get the plugin name
    fn name(&self) -> &str;

    /// Evaluate the supplied [`StreamContext`] for this lifecycle invocation.
    async fn on_connection(&self, ctx: &StreamContext) -> StreamPluginResult;
}

#[cfg(test)]
mod tests {
    use super::StreamLayer;

    #[test]
    fn stream_layer_metric_labels_are_stable() {
        assert_eq!(StreamLayer::Stage1.as_metric_label(), "stage1");
        assert_eq!(StreamLayer::Tcp.as_metric_label(), "tcp");
    }
}
