//! TLS route plugin trait and context definitions.
//!
//! These types are used for **Stage 2** of the stream plugin pipeline:
//! after TLS handshake and route matching, before backend connect.
//! Unlike the ConnectionFilter stage (Stage 1) which only has client IP,
//! this stage has full TLS context including SNI and matched route info.

use async_trait::async_trait;
use std::net::IpAddr;

use super::super::stream_plugin_trait::StreamPluginResult;
use crate::observe::logs::LogBuffer;

/// Enriched context available after TLS handshake + TLSRoute match.
#[derive(Debug, Clone)]
pub struct TlsRouteContext {
    pub direct_peer_ip: IpAddr,
    pub remote_ip: IpAddr,
    pub listener_port: u16,
    pub sni: String,
    pub tls_id: Option<String>,
    pub rsc: crate::Rsc,
    pub is_mtls: bool,
    /// Per-connection plugin log buffer; callers may leave as `LogBuffer::new()`.
    pub log: LogBuffer,
}

impl TlsRouteContext {
    /// Push a plain log entry into the context log buffer.
    #[inline]
    pub fn push(&mut self, msg: &str) {
        let _ = self.log.push(msg);
    }

    /// Push an error entry (static kind only) with unified `"err: "` prefix.
    #[inline]
    pub fn push_err(&mut self, kind: &'static str) {
        let _ = self.log.push_err(kind);
    }
}

/// Plugin trait for the TLS route stage (post-handshake, post-route-match).
#[async_trait]
pub trait TlsRoutePlugin: Send + Sync {
    fn name(&self) -> &str;

    /// Execute plugin logic after TLS route matching.
    /// Returns Allow to proceed or Deny to reject the connection.
    async fn on_tls_route(&self, ctx: &TlsRouteContext) -> StreamPluginResult;
}
