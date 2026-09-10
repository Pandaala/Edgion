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
use crate::plugins::{StreamPluginResult, TlsRouteContext};
use crate::routes::common::{l4_post_select, L4PostSelect};
use crate::routes::l4_duplex::{DuplexError, DuplexHooks};
use edgion_resources::resources::common::RefDenied;
use proxy::TlsContext;

/// Shared TLS retry-loop gate. Both terminate and passthrough must
/// `return` when this is true. The helper has no retry/continue outcome:
/// authorization and kind misses are not connect failures.
pub(super) fn apply_tls_l4_post_select(
    ctx: &mut TlsContext,
    ref_denied: Option<&RefDenied>,
    group: Option<&str>,
    kind: Option<&str>,
) -> bool {
    match l4_post_select(ref_denied, group, kind) {
        L4PostSelect::FailClosed { status, log } => {
            ctx.status = Some(status);
            ctx.err_log = Some(log.to_string());
            true
        }
        L4PostSelect::Proceed => false,
    }
}

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
/// Negotiated TLS identity and mTLS state are available only in terminate mode.
#[derive(Clone, Copy)]
pub(super) enum TlsRouteMode {
    Terminated,
    Passthrough,
}

pub(super) async fn run_tls_route_plugins(
    store_key: Option<&str>,
    ctx: &mut TlsContext,
    sni: &str,
    listener_port: u16,
    mode: TlsRouteMode,
) -> bool {
    let Some(store_key) = store_key else {
        return true;
    };
    // Fail-close: if either canonical address cannot be parsed (for example,
    // the literal "unknown" because peer extraction failed), an IP-allowlist
    // policy referenced by the rule cannot be enforced. Admitting the
    // connection here would silently bypass that policy.
    let Ok(direct_peer_ip) = ctx.direct_peer_ip.parse() else {
        ctx.err_log = Some(format!(
            "direct peer IP '{}' could not be parsed; rejecting (fail-close)",
            ctx.direct_peer_ip,
        ));
        return false;
    };
    let Ok(remote_ip) = ctx.remote_ip.parse() else {
        ctx.err_log = Some(format!(
            "remote IP '{}' could not be parsed; rejecting (fail-close)",
            ctx.remote_ip,
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

    // A Stage-1-only attachment cannot express TLSRoute policy. Check the
    // declaration, not compiled runtime emptiness: disabled and Bypass entries
    // must not silently select another stage. An explicit empty Stage 2 is a no-op.
    if resource.spec.spec.tls_route_plugins.is_none() && resource.spec.has_plugins() {
        ctx.err_log = Some(format!(
            "stream plugin resource '{store_key}' requires tlsRoutePlugins for TLSRoute; rejecting (fail-close)"
        ));
        return false;
    }

    // TLSRoute dispatch runs only the Stage-2 runtime.
    let tls_runtime = &resource.tls_route_plugin_runtime;
    if !tls_runtime.is_empty() {
        let result = {
            let fallback_rsc;
            let rsc = match ctx.matched.as_ref() {
                Some(matched) => &matched.rsc,
                None => {
                    fallback_rsc = crate::Rsc::new("TLSRoute", "", "");
                    &fallback_rsc
                }
            };
            let (tls_id, is_mtls) = match mode {
                TlsRouteMode::Terminated => (ctx.tls_id.as_deref(), ctx.is_mtls),
                TlsRouteMode::Passthrough => (None, false),
            };
            let tls_ctx = TlsRouteContext {
                direct_peer_ip,
                remote_ip,
                listener_port,
                sni,
                tls_id,
                rsc,
                is_mtls,
            };
            tls_runtime.run(&tls_ctx).await
        };
        return match result {
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

    true
}

pub use gateway_tls_routes::TlsRouteTable;
pub use route_unit::TlsRouteUnit;

pub use passthrough_proxy::EdgionTlsPassthroughProxy;
pub use proxy::EdgionTlsTcpProxy;

#[cfg(test)]
mod tests;
