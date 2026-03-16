//! TLS route plugin trait and context definitions.
//!
//! These types are used for **Stage 2** of the stream plugin pipeline:
//! after TLS handshake and route matching, before backend connect.
//! Unlike the ConnectionFilter stage (Stage 1) which only has client IP,
//! this stage has full TLS context including SNI and matched route info.

use async_trait::async_trait;
use std::net::IpAddr;

use super::super::stream_plugin_trait::StreamPluginResult;

/// Enriched context available after TLS handshake + TLSRoute match.
#[derive(Debug, Clone)]
pub struct TlsRouteContext {
    pub client_ip: IpAddr,
    pub listener_port: u16,
    pub sni: String,
    pub tls_id: Option<String>,
    pub matched_route_ns: String,
    pub matched_route_name: String,
    pub is_mtls: bool,
}

/// Plugin trait for the TLS route stage (post-handshake, post-route-match).
#[async_trait]
pub trait TlsRoutePlugin: Send + Sync {
    fn name(&self) -> &str;

    /// Execute plugin logic after TLS route matching.
    /// Returns Allow to proceed or Deny to reject the connection.
    async fn on_tls_route(&self, ctx: &TlsRouteContext) -> StreamPluginResult;
}
