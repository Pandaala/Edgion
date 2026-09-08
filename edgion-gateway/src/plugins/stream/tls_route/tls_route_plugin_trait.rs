//! TLS route plugin trait and context definitions.
//!
//! These types are used after TLSRoute matching in both proxy modes. Terminated
//! TLS runs post-handshake with TLS identity and actual mTLS state. Passthrough
//! TLS runs after ClientHello/SNI inspection and before forwarding the
//! end-to-end handshake, with no TLS identity and `is_mtls` set to false.

use async_trait::async_trait;
use std::net::IpAddr;

use super::super::stream_plugin_trait::StreamPluginResult;

/// TLSRoute match context; TLS identity and mTLS state are available only in terminated mode.
#[derive(Debug, Clone)]
pub struct TlsRouteContext<'a> {
    pub direct_peer_ip: IpAddr,
    pub remote_ip: IpAddr,
    pub listener_port: u16,
    pub sni: &'a str,
    pub tls_id: Option<&'a str>,
    pub rsc: &'a crate::Rsc,
    pub is_mtls: bool,
}

/// Plugin trait evaluated after TLSRoute matching in terminate and passthrough modes.
#[async_trait]
pub trait TlsRoutePlugin: Send + Sync {
    /// Execute plugin logic after TLS route matching.
    /// Returns Allow to proceed or Deny to reject the connection.
    async fn on_tls_route(&self, ctx: &TlsRouteContext<'_>) -> StreamPluginResult;
}
