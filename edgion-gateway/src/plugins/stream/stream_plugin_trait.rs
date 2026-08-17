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
    /// Client IP address
    pub client_ip: IpAddr,
    /// Listener port that received the connection
    pub listener_port: u16,
    /// Optional: remote address string (for logging)
    pub remote_addr: Option<String>,
    /// Construction-site label for the `layer` metric dimension: `"stage1"` |
    /// `"tcp"` | `"tls"`. Defaults to `"stage1"` so the 20+ existing `new()`
    /// call sites (mostly tests) do not need updating; the three production
    /// call sites that are not stage1 set it explicitly via `with_layer`.
    pub layer: &'static str,
}

impl StreamContext {
    /// Create a new stream context.
    ///
    /// `layer` defaults to `"stage1"`, which is correct ONLY for the
    /// connection-filter stage. **Any new production call site that runs after
    /// the PROXY-protocol handshake — a TCP, TLS, or future route stage — MUST
    /// chain `.with_layer(...)`.** Forgetting to is silent: the context still
    /// builds, and `edgion_geoip_denied_total` simply attributes that stage's
    /// denials to `stage1`. Since L4 geo denials are never logged, that metric is
    /// the only record of them, so a wrong label is a wrong answer with nothing
    /// to cross-check it against.
    pub fn new(client_ip: IpAddr, listener_port: u16) -> Self {
        Self {
            client_ip,
            listener_port,
            remote_addr: None,
            layer: "stage1",
        }
    }

    /// Override the `layer` label used for the `edgion_geoip_denied_total` metric.
    pub fn with_layer(mut self, layer: &'static str) -> Self {
        self.layer = layer;
        self
    }
}

/// Stream plugin trait for TCP/UDP connection filtering
#[async_trait]
pub trait StreamPlugin: Send + Sync {
    /// Get the plugin name
    fn name(&self) -> &str;

    /// Execute plugin logic for a new TCP/UDP connection.
    async fn on_connection(&self, ctx: &StreamContext) -> StreamPluginResult;
}
