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
mod routes_mgr;

// TLS proxy modules
pub mod passthrough_proxy;
pub mod proxy;
pub mod sni_parser;

pub use routes_mgr::{get_global_tls_route_managers, GlobalTlsRouteManagers, TlsRouteManager, TlsRouteManagerStats};

pub use conf_handler_impl::create_tls_route_handler;

use crate::core::gateway::plugins::stream::get_global_stream_plugin_store;
use crate::core::gateway::plugins::{StreamContext, StreamPluginResult, TlsRouteContext};
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

/// Shared Stage 2 TLS route plugin execution.
///
/// Used by both terminate and passthrough proxy.
/// `tls_id` and `is_mtls` differ by mode: passthrough passes `None`/`false`.
pub(super) async fn run_tls_route_plugins(
    rule: &crate::types::resources::tls_route::TLSRouteRule,
    ctx: &mut TlsContext,
    sni: &str,
    listener_port: u16,
    tls_id: Option<String>,
    is_mtls: bool,
) -> bool {
    let Some(store_key) = &rule.stream_plugin_store_key else {
        return true;
    };
    let Ok(client_ip) = ctx.client_addr.parse() else {
        return true;
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
    let tls_runtime = &resource.spec.tls_route_plugin_runtime;
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
                .unwrap_or_else(|| crate::types::Rsc {
                    kind: "TLSRoute".to_string(),
                    ns: String::new(),
                    name: String::new(),
                }),
            is_mtls,
            log: crate::core::gateway::observe::logs::LogBuffer::new(),
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
    let stream_runtime = &resource.spec.stream_plugin_runtime;
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

// Export TLS proxy types
pub use passthrough_proxy::EdgionTlsPassthroughProxy;
pub use proxy::EdgionTlsTcpProxy;

#[cfg(test)]
mod fail_close_tests {
    use super::*;
    use crate::core::gateway::lb::BackendSelector;
    use crate::types::resources::tls_route::TLSRouteRule;

    fn empty_rule_with_store_key(key: Option<String>) -> TLSRouteRule {
        TLSRouteRule {
            backend_refs: None,
            backend_finder: BackendSelector::new(),
            plugin_runtime: Default::default(),
            proxy_protocol_version: None,
            stream_plugin_store_key: key,
            max_connect_retries: 1,
        }
    }

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

        let rule = empty_rule_with_store_key(Some(key.clone()));
        let mut ctx = TlsContext::new(443, "127.0.0.1".to_string(), 50000);

        let allowed = run_tls_route_plugins(&rule, &mut ctx, "example.com", 443, None, false).await;
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
        let rule = empty_rule_with_store_key(None);
        let mut ctx = TlsContext::new(443, "127.0.0.1".to_string(), 50000);
        let allowed = run_tls_route_plugins(&rule, &mut ctx, "example.com", 443, None, false).await;
        assert!(allowed, "rule with no store_key must pass through");
        assert!(ctx.err_log.is_none());
    }

    #[tokio::test]
    async fn admit_when_client_addr_unparseable() {
        // Rule references an ESP but client_addr is not a valid IP.
        // Pre-existing behavior: pass through (kept unchanged by fail-close fix).
        let rule = empty_rule_with_store_key(Some("default/anything".to_string()));
        let mut ctx = TlsContext::new(443, "unknown".to_string(), 0);
        let allowed = run_tls_route_plugins(&rule, &mut ctx, "example.com", 443, None, false).await;
        assert!(allowed, "unparseable client_addr keeps existing pass-through");
    }
}
