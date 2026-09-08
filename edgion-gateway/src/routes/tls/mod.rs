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
use crate::plugins::{StreamContext, StreamLayer, StreamPluginResult, TlsRouteContext};
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
    // Fail-close: if either canonical address cannot be parsed
    // to the literal "unknown" because peer extraction failed), an IP-allowlist
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

    // Prefer Stage 2 (TlsRoute) runtime
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

    // Fall back to Stage 1 (ConnectionFilter) runtime
    let stream_runtime = &resource.stream_plugin_runtime;
    if !stream_runtime.is_empty() {
        let stream_ctx = StreamContext::new(direct_peer_ip, listener_port, StreamLayer::Tls);
        return match stream_runtime.run(&stream_ctx).await {
            StreamPluginResult::Allow => {
                ctx.push_log("stream plugin allowed (compat)");
                true
            }
            StreamPluginResult::Deny(reason) => {
                // Design spec §6.6: an L4 geo denial writes no log entry at any
                // stage and records no client address — the metric
                // (`edgion_geoip_denied_total`, already emitted by the plugin
                // itself) is the only signal, precisely to deny a
                // country-blocked scanner the amplification of "does logging
                // this connection cost the gateway more than dropping it".
                // `ctx.err_log` is emitted alongside `direct_peer_ip` in the same
                // JSON object (`proxy.rs`), so a geo reason must never reach
                // it — collapse it the way the TCP path
                // (`routes/tcp/edgion_tcp.rs::check_stream_plugins`) already
                // does for every Stage 1 reason, scoped here to geo reasons
                // only so a non-geo IP-restriction denial keeps its detail.
                ctx.err_log = Some(if is_geo_deny_reason(&reason) {
                    "stream plugin denied (compat)".to_string()
                } else {
                    format!("stream plugin denied (compat): {reason}")
                });
                false
            }
        };
    }

    true
}

/// `true` for the bounded `"geo:deny:*"` reasons emitted by
/// `GlobalConnectionIpRestriction`'s geo tier (see
/// `plugins/stream/global_connection_ip_restriction/plugin.rs`). Any other
/// Stage 1 deny reason (e.g. a plain IP-restriction hit) is unaffected.
fn is_geo_deny_reason(reason: &str) -> bool {
    reason.starts_with("geo:deny:")
}

pub use gateway_tls_routes::TlsRouteTable;
pub use route_unit::TlsRouteUnit;

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

        let mut ctx = TlsContext::new(443, "127.0.0.1".to_string(), 50000, "127.0.0.1".to_string(), 50000);

        let allowed = run_tls_route_plugins(
            Some(key.as_str()),
            &mut ctx,
            "example.com",
            443,
            TlsRouteMode::Passthrough,
        )
        .await;
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
        let mut ctx = TlsContext::new(443, "127.0.0.1".to_string(), 50000, "127.0.0.1".to_string(), 50000);
        let allowed = run_tls_route_plugins(None, &mut ctx, "example.com", 443, TlsRouteMode::Passthrough).await;
        assert!(allowed, "rule with no store_key must pass through");
        assert!(ctx.err_log.is_none());
    }

    #[tokio::test]
    async fn reject_when_direct_peer_ip_unparseable() {
        // Rule references an ESP but DirectPeerIp is not a valid IP (e.g. the
        // literal "unknown" set when Pingora cannot extract the peer addr).
        // Without a parseable client IP we cannot evaluate an IP-allowlist
        // policy, so the connection must be rejected (fail-close).
        let mut ctx = TlsContext::new(443, "unknown".to_string(), 0, "unknown".to_string(), 0);
        let allowed = run_tls_route_plugins(
            Some("default/anything"),
            &mut ctx,
            "example.com",
            443,
            TlsRouteMode::Passthrough,
        )
        .await;
        assert!(!allowed, "unparseable DirectPeerIp must fail-close");
        let err = ctx.err_log.as_deref().unwrap_or("");
        assert!(
            err.contains("fail-close"),
            "err_log should mention fail-close, got: {err}"
        );
        assert!(
            err.contains("direct peer IP"),
            "err_log should mention DirectPeerIp, got: {err}"
        );
    }

    #[test]
    fn apply_tls_l4_post_select_denied_sets_status_and_is_terminal() {
        let mut ctx = TlsContext::new(443, "127.0.0.1".to_string(), 50000, "127.0.0.1".to_string(), 50000);
        let denied = RefDenied {
            target_namespace: "other".to_string(),
            target_name: "svc".to_string(),
            reason: Some("NoMatchingReferenceGrant".to_string()),
        };
        assert!(apply_tls_l4_post_select(
            &mut ctx,
            Some(&denied),
            Some("apps"),
            Some("Deployment")
        ));
        assert_eq!(ctx.status, Some(crate::routes::common::L4_BACKEND_REF_DENIED_STATUS));
        assert_eq!(
            ctx.err_log.as_deref(),
            Some(crate::routes::common::L4_BACKEND_REF_DENIED_LOG)
        );
        let json = serde_json::to_value(&ctx).expect("TlsContext serializes");
        assert_eq!(json["status"], "RefDenied");
        assert_eq!(json["err_log"], "backend ref denied");
    }

    #[test]
    fn apply_tls_l4_post_select_unsupported_kind_sets_status_and_is_terminal() {
        let mut ctx = TlsContext::new(443, "127.0.0.1".to_string(), 50000, "127.0.0.1".to_string(), 50000);
        assert!(apply_tls_l4_post_select(
            &mut ctx,
            None,
            Some("apps"),
            Some("Deployment")
        ));
        assert_eq!(
            ctx.status,
            Some(crate::routes::common::L4_BACKEND_UNSUPPORTED_KIND_STATUS)
        );
        assert_eq!(
            ctx.err_log.as_deref(),
            Some(crate::routes::common::L4_BACKEND_UNSUPPORTED_KIND_LOG)
        );
        let json = serde_json::to_value(&ctx).expect("TlsContext serializes");
        assert_eq!(json["status"], "UnsupportedBackend");
        assert_eq!(json["err_log"], "unsupported backend kind");
    }

    #[test]
    fn apply_tls_l4_post_select_core_service_is_not_terminal() {
        let mut ctx = TlsContext::new(443, "127.0.0.1".to_string(), 50000, "127.0.0.1".to_string(), 50000);
        assert!(!apply_tls_l4_post_select(&mut ctx, None, None, None));
        assert!(ctx.status.is_none());
        assert!(ctx.err_log.is_none());
        let json = serde_json::to_value(&ctx).expect("TlsContext serializes");
        assert!(json.get("status").is_none());
    }

    #[test]
    fn is_geo_deny_reason_matches_only_the_bounded_geo_prefix() {
        assert!(is_geo_deny_reason("geo:deny:country"));
        assert!(is_geo_deny_reason("geo:deny:allow-miss"));
        assert!(is_geo_deny_reason("geo:deny:db-unavailable"));
        assert!(!is_geo_deny_reason("stream plugin policy denied"));
        assert!(!is_geo_deny_reason("ip restriction denied"));
    }

    // The former `tls_stage1_compat_geo_denial_never_logs_the_geo_reason`
    // regression test drove a real `GlobalConnectionIpRestriction` geo block
    // through this path. GCIR's geo tier was extracted (see the
    // `2026-08-15-geoip-country-restriction` task); `is_geo_deny_reason`'s
    // bounded-prefix filter above stays as-is (generic string matching, no
    // GCIR dependency) for the forthcoming `GeoIpLocation` plugin to reuse,
    // but the GCIR-specific construction this test used no longer compiles.
    // Coverage will be restored against the new plugin's deny reason.
}
