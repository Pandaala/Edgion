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
}

impl StreamContext {
    /// Create a new stream context
    pub fn new(client_ip: IpAddr, listener_port: u16) -> Self {
        Self {
            client_ip,
            listener_port,
            remote_addr: None,
        }
    }

    /// Create a stream context with remote address
    pub fn with_remote_addr(client_ip: IpAddr, listener_port: u16, remote_addr: String) -> Self {
        Self {
            client_ip,
            listener_port,
            remote_addr: Some(remote_addr),
        }
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
