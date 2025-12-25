//! Stream plugin trait definition

use async_trait::async_trait;
use std::net::IpAddr;

/// Result of stream plugin execution
#[derive(Debug, Clone)]
pub enum StreamPluginResult {
    /// Allow the connection to proceed
    Allow,
    /// Deny the connection with a reason
    Deny(String),
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

    /// Execute plugin logic on new connection
    /// Called when a new TCP/UDP connection is established
    async fn on_connection(&self, ctx: &StreamContext) -> StreamPluginResult;
}

