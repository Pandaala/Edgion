//! TLS Routes and Proxy Implementation
//!
//! This module contains all TLS-related functionality:
//! - Route matching and management ([`routes_mgr`])
//! - TLS proxy implementation ([`proxy`]) — Terminate mode
//! - TLS passthrough proxy ([`passthrough_proxy`]) — Passthrough mode
//! - ClientHello SNI extraction ([`sni_parser`])
//! - Per-port route table ([`gateway_tls_routes`])

use std::time::Duration;

mod conf_handler_impl;
pub(crate) mod gateway_tls_routes;
mod route_unit;
mod routes_mgr;

// TLS proxy modules
pub mod passthrough_proxy;
pub mod proxy;
pub mod sni_parser;

pub use routes_mgr::{get_global_tls_route_managers, GlobalTlsRouteManagers, TlsRouteManager, TlsRouteManagerStats};

pub use conf_handler_impl::create_tls_route_handler;

use crate::plugins::stream::get_global_stream_plugin_store;
use crate::plugins::{StreamContext, StreamPluginResult, TlsRouteContext};
use crate::routes::l4_duplex::{DuplexError, DuplexHooks};
use proxy::TlsContext;

pub(super) const MAX_TLS_CONNECT_RETRIES: u32 = 5;
pub(super) const DEFAULT_TLS_CONNECT_RETRIES: u32 = 2;
const TLS_CONNECT_RETRY_BACKOFF_BASE_MS: u64 = 25;
const TLS_CONNECT_RETRY_BACKOFF_MAX_MS: u64 = 250;

pub(super) fn cap_tls_connect_retries(attempts: u32) -> u32 {
    attempts.clamp(1, MAX_TLS_CONNECT_RETRIES)
}

pub(super) fn tls_connect_retry_backoff(attempt: u32) -> Duration {
    let multiplier = 1u64.checked_shl(attempt.min(31)).unwrap_or(u64::MAX);
    let millis = TLS_CONNECT_RETRY_BACKOFF_BASE_MS
        .saturating_mul(multiplier)
        .min(TLS_CONNECT_RETRY_BACKOFF_MAX_MS);
    Duration::from_millis(millis)
}

/// Shared [`DuplexHooks`] for both TLS proxy modes (terminate and passthrough).
///
/// - Byte counters live on the per-attempt `TlsUpstreamInfo` reached via
///   `current_upstream_mut()`; if no upstream is currently being attempted
///   the increment is silently skipped (matches prior behavior).
/// - Errors surface as a free-form `ctx.err_log` string; `DownstreamFlush`
///   uses a distinct `"downstream flush err"` message (matches prior behavior).
pub(super) struct TlsDuplexHooks<'a> {
    pub ctx: &'a mut TlsContext,
}

impl DuplexHooks for TlsDuplexHooks<'_> {
    fn record_sent(&mut self, n: usize) {
        if let Some(info) = self.ctx.current_upstream_mut() {
            info.bytes_sent += n as u64;
        }
    }

    fn record_received(&mut self, n: usize) {
        if let Some(info) = self.ctx.current_upstream_mut() {
            info.bytes_received += n as u64;
        }
    }

    fn on_error(&mut self, err: DuplexError) {
        let msg = match err {
            DuplexError::UpstreamWrite => "upstream write err",
            DuplexError::DownstreamRead => "downstream read err",
            DuplexError::DownstreamWrite => "downstream write err",
            DuplexError::DownstreamFlush => "downstream flush err",
            DuplexError::UpstreamRead => "upstream read err",
            DuplexError::IdleTimeout => "idle timeout",
        };
        self.ctx.err_log = Some(msg.to_string());
    }
}

/// Shared Stage 2 TLS route plugin execution.
///
/// Used by both terminate and passthrough proxy.
/// `tls_id` and `is_mtls` differ by mode: passthrough passes `None`/`false`.
pub(super) async fn run_tls_route_plugins(
    store_key: Option<&str>,
    ctx: &mut TlsContext,
    sni: &str,
    listener_port: u16,
    tls_id: Option<String>,
    is_mtls: bool,
) -> bool {
    let Some(store_key) = store_key else {
        return true;
    };
    // Fail-close: if the client address cannot be parsed (e.g. Pingora set it
    // to the literal "unknown" because peer extraction failed), an IP-allowlist
    // policy referenced by the rule cannot be enforced. Admitting the
    // connection here would silently bypass that policy.
    let Ok(client_ip) = ctx.client_addr.parse() else {
        ctx.err_log = Some(format!(
            "client address '{}' could not be parsed; rejecting (fail-close)",
            ctx.client_addr,
        ));
        return false;
    };

    let store = get_global_stream_plugin_store();
    let Some(resource) = store.get(store_key) else {
        // Fail-close: TLSRoute rule references an ESP but it is not in the
        // store. The policy (typically an IP allowlist) cannot be enforced;
        // silently admitting traffic would be a security bypass. Startup-time
        // sync race is gated by `ConfigClient::is_ready()` before listeners
        // bind, so reaching this branch means misconfiguration or hot-delete.
        ctx.err_log = Some(format!(
            "stream plugin resource '{store_key}' not found; rejecting (fail-close)"
        ));
        return false;
    };

    // Prefer Stage 2 (TlsRoute) runtime
    let tls_runtime = &resource.tls_route_plugin_runtime;
    if !tls_runtime.is_empty() {
        let mut tls_ctx = TlsRouteContext {
            client_ip,
            listener_port,
            sni: sni.to_string(),
            tls_id,
            rsc: ctx
                .matched
                .as_ref()
                .map(|m| m.rsc.clone())
                .unwrap_or_else(|| crate::Rsc::new("TLSRoute", "", "")),
            is_mtls,
            log: crate::observe::logs::LogBuffer::new(),
        };
        return match tls_runtime.run(&mut tls_ctx).await {
            StreamPluginResult::Allow => {
                ctx.push_log("tls route plugin allowed");
                true
            }
            StreamPluginResult::Deny(reason) => {
                ctx.err_log = Some(format!("tls route plugin denied: {reason}"));
                false
            }
        };
    }

    // Backward compat: fall back to Stage 1 (ConnectionFilter) runtime
    let stream_runtime = &resource.stream_plugin_runtime;
    if !stream_runtime.is_empty() {
        let stream_ctx = StreamContext::new(client_ip, listener_port);
        return match stream_runtime.run(&stream_ctx).await {
            StreamPluginResult::Allow => {
                ctx.push_log("stream plugin allowed (compat)");
                true
            }
            StreamPluginResult::Deny(reason) => {
                ctx.err_log = Some(format!("stream plugin denied (compat): {reason}"));
                false
            }
        };
    }

    true
}

pub use gateway_tls_routes::TlsRouteTable;
pub use route_unit::TlsRouteUnit;

// Export TLS proxy types
pub use passthrough_proxy::EdgionTlsPassthroughProxy;
pub use proxy::EdgionTlsTcpProxy;

#[cfg(test)]
mod fail_close_tests {
    use super::*;

    #[tokio::test]
    async fn fail_close_when_store_key_not_in_store() {
        // Stage-2 fail-close: rule references "default/missing" but store is
        // empty. Must reject and surface an err_log entry.
        let store = get_global_stream_plugin_store();
        // Avoid touching other tests' state: make sure this key is not present.
        // (The global store is shared; we only assert the missing-key path,
        // which never inserts anything.)
        let key = "default/__fail_close_missing__".to_string();
        assert!(store.get(&key).is_none(), "test precondition: key must not be in store");

        let mut ctx = TlsContext::new(443, "127.0.0.1".to_string(), 50000);

        let allowed = run_tls_route_plugins(Some(key.as_str()), &mut ctx, "example.com", 443, None, false).await;
        assert!(!allowed, "missing ESP must fail-close");
        let err = ctx.err_log.as_deref().unwrap_or("");
        assert!(
            err.contains("fail-close"),
            "err_log should mention fail-close, got: {err}"
        );
        assert!(
            err.contains(&key),
            "err_log should include the missing store key, got: {err}"
        );
    }

    #[tokio::test]
    async fn admit_when_store_key_is_none() {
        // No ESP attached to the rule — intentional pass-through, must allow.
        let mut ctx = TlsContext::new(443, "127.0.0.1".to_string(), 50000);
        let allowed = run_tls_route_plugins(None, &mut ctx, "example.com", 443, None, false).await;
        assert!(allowed, "rule with no store_key must pass through");
        assert!(ctx.err_log.is_none());
    }

    #[tokio::test]
    async fn reject_when_client_addr_unparseable() {
        // Rule references an ESP but client_addr is not a valid IP (e.g. the
        // literal "unknown" set when Pingora cannot extract the peer addr).
        // Without a parseable client IP we cannot evaluate an IP-allowlist
        // policy, so the connection must be rejected (fail-close).
        let mut ctx = TlsContext::new(443, "unknown".to_string(), 0);
        let allowed = run_tls_route_plugins(Some("default/anything"), &mut ctx, "example.com", 443, None, false).await;
        assert!(!allowed, "unparseable client_addr must fail-close");
        let err = ctx.err_log.as_deref().unwrap_or("");
        assert!(
            err.contains("fail-close"),
            "err_log should mention fail-close, got: {err}"
        );
        assert!(
            err.contains("client address"),
            "err_log should mention 'client address', got: {err}"
        );
    }
}
