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

/// Render the known destination once under the log policy, including copies
/// embedded in Pingora's error chain. Keep the surrounding diagnostic intact.
fn format_connect_error(address: &str, error: &pingora_core::Error) -> String {
    let detail = format!("connect failed: {address}: {error}");
    if address.is_empty() || !edgion_resources::sensitive::is_enabled() {
        return detail;
    }
    edgion_resources::sensitive::apply_sensitive(address, |masked| detail.replace(address, masked))
}

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

#[cfg(test)]
mod sensitive_log_tests {
    use super::*;
    use edgion_resources::sensitive::{init_sensitive, SensitiveMaskConfig, SensitiveMode};
    use pingora_core::upstreams::peer::BasicPeer;
    use proxy::TlsUpstreamInfo;
    use serde_json::Value;

    pub(super) fn assert_log_modes(test_name: &str, listener: impl Fn(&TlsContext) -> Value) {
        const CHILD: &str = "EDGION_TLS_ADDRESS_LOG_CHILD";
        const MODE: &str = "EDGION_TLS_ADDRESS_LOG_MODE";
        let test_name = test_name.strip_prefix("edgion_gateway::").unwrap();
        if std::env::var(CHILD).as_deref() != Ok(test_name) {
            for mode in ["off", "replace", "wrap"] {
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", test_name, "--nocapture"])
                    .env(CHILD, test_name)
                    .env(MODE, mode)
                    .output()
                    .expect("isolated TLS log test must start");
                assert!(
                    output.status.success(),
                    "{mode}: {}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
            }
            return;
        }
        let mode = match std::env::var(MODE).unwrap().as_str() {
            "off" => SensitiveMode::Off,
            "replace" => SensitiveMode::Replace,
            "wrap" => SensitiveMode::Wrap,
            _ => panic!("unexpected isolated test mode"),
        };
        init_sensitive(SensitiveMaskConfig {
            mode,
            replace_with: "[hidden]".into(),
            wrap_prefix: "<mask>".into(),
            wrap_suffix: "</mask>".into(),
        });
        // SslLogger serializes this handshake metadata directly. Its SNI and
        // nonsensitive diagnostics are intentionally outside address masking.
        let mut meta = crate::TlsConnMeta {
            ts: 0,
            start_at: std::time::Instant::now(),
            handshake_complete_time: Some(1),
            port: Some(443),
            tls_id: None,
            sni: Some("public.example.com".into()),
            log: None,
            matched: None,
            err_log: Some("cert_expired: not_after=Jan 1 00:00:00 2026 GMT".into()),
            client_cert_info: None,
            is_mtls: false,
        };
        let ssl_json = serde_json::to_value(&meta).unwrap();
        assert_eq!(ssl_json["sni"], "public.example.com");
        assert_eq!(ssl_json["err_log"], "cert_expired: not_after=Jan 1 00:00:00 2026 GMT");
        meta.sni = None;
        meta.err_log = None;
        let ssl_json = serde_json::to_value(&meta).unwrap();
        assert!(ssl_json.get("sni").is_none());
        assert!(ssl_json.get("err_log").is_none());
        for address in ["192.0.2.40:8443", "[2001:db8::40]:8443"] {
            let socket: std::net::SocketAddr = address.parse().unwrap();
            let ip = socket.ip().to_string();
            let peer = BasicPeer::new(address);
            // Reproduce the actual nested Pingora connector error shape.
            let cause = pingora_core::Error::because(
                pingora_core::ErrorType::ConnectError,
                format!("Fail to connect to {address}"),
                std::io::Error::from(std::io::ErrorKind::ConnectionRefused),
            );
            let error = cause.more_context(format!("Fail to connect to {peer}"));
            let diagnostic = format_connect_error(address, &error);
            assert!(diagnostic.contains("ConnectError"));
            assert!(diagnostic.contains("connection refused"));
            assert!(diagnostic.starts_with("connect failed: "));
            let expected_ip = match mode {
                SensitiveMode::Off => {
                    assert_eq!(diagnostic, format!("connect failed: {address}: {error}"));
                    ip.clone()
                }
                SensitiveMode::Replace => {
                    assert!(!diagnostic.contains(&ip), "{diagnostic}");
                    assert_eq!(diagnostic.matches("[hidden]").count(), 3);
                    "[hidden]".into()
                }
                SensitiveMode::Wrap => {
                    assert_eq!(diagnostic.matches(&format!("<mask>{address}</mask>")).count(), 3);
                    format!("<mask>{ip}</mask>")
                }
            };
            let mut ctx = TlsContext::new(443, "192.0.2.1".into(), 1234, "192.0.2.1".into(), 1234);
            ctx.sni = Some("public.example.com".into());
            ctx.err_log = Some(diagnostic.clone());
            ctx.push_log(&format!("retry 1/2: {diagnostic}"));
            ctx.upstream_info.push(TlsUpstreamInfo {
                addr: Some(ip.clone()),
                port: Some(socket.port()),
                backend: None,
                bytes_sent: 0,
                bytes_received: 0,
                connect_time: None,
                connection_established: false,
                proxy_protocol_sent: false,
                upstream_protocol: Some("TCP".into()),
            });
            for json in [serde_json::to_value(&ctx).unwrap(), listener(&ctx)] {
                assert_eq!(json["sni"], "public.example.com");
                assert_eq!(json["err_log"], diagnostic);
                // LogBuffer may reject a long entry; accepted entries preserve diagnostics.
                for entry in json["log"].as_array().unwrap() {
                    assert_eq!(entry, &format!("retry 1/2: {diagnostic}"));
                }
                assert_eq!(json["upstream_info"][0]["addr"], expected_ip);
                assert_eq!(json["upstream_info"][0]["port"], 8443);
            }
            assert_eq!(ctx.upstream_info[0].addr.as_deref(), Some(ip.as_str()));
            assert_eq!(ctx.sni.as_deref(), Some("public.example.com"));
            // Short retry entries must survive the byte cap and must not be masked twice.
            let short = format_connect_error(
                address,
                &pingora_core::Error::new(pingora_core::ErrorType::ConnectTimedout),
            );
            ctx.log = None;
            ctx.push_log(&format!("retry 1/2: {short}"));
            assert_eq!(listener(&ctx)["log"][0], format!("retry 1/2: {short}"));
            for message in ["idle timeout", "no endpoint: default/service", "no SNI"] {
                ctx.err_log = Some(message.into());
                assert_eq!(listener(&ctx)["err_log"], message);
                assert_eq!(serde_json::to_value(&ctx).unwrap()["err_log"], message);
            }
            ctx.err_log = None;
            ctx.sni = None;
            ctx.upstream_info[0].addr = None;
            let global = serde_json::to_value(&ctx).unwrap();
            assert!(global.get("sni").is_none());
            assert!(global.get("err_log").is_none());
            let local = listener(&ctx);
            assert!(local["sni"].is_null());
            assert!(local["err_log"].is_null());
            for json in [global, local] {
                assert!(json["upstream_info"][0].get("addr").is_none());
            }
        }
    }
}
