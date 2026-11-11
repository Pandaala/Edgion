//! TLS Routes and Proxy Implementation
//!
//! This module contains all TLS-related functionality:
//! - Route matching and management ([`routes_mgr`])
//! - TLS proxy implementation ([`proxy`]) — Terminate mode
//! - TLS passthrough proxy ([`passthrough_proxy`]) — Passthrough mode
//! - ClientHello SNI extraction ([`sni_parser`])
//! - Per-port route table ([`gateway_tls_routes`])

use crate::plugins::publication::{PluginPublication, PluginScope};
use edgion_resources::ListenerKey;
use std::sync::Arc;
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

/// An eligible Route has already won. Missing policy is terminal, not no-match.
struct SelectedTlsRoute {
    owner: Arc<crate::runtime::gateway_info::GatewayInfo>,
    unit: Arc<TlsRouteUnit>,
    scope: Option<Arc<PluginScope>>,
}

/// Both proxy modes bind attachment, owner and policy against one publication.
/// Retain only selected handles, never table/publication guards across relay.
fn select_tls_route(
    manager: &TlsRouteManager,
    publication: &PluginPublication,
    listener_key: &ListenerKey,
    sni: &str,
) -> Option<SelectedTlsRoute> {
    let index = publication.listener_index(listener_key)?;
    let (unit, owner) = manager.load_route_table().select_route(sni, index)?;
    let scope = publication
        .gateway(owner.gateway_key())
        .filter(|gateway| gateway.scope.available)
        .map(|gateway| Arc::clone(&gateway.scope));
    Some(SelectedTlsRoute { unit, scope, owner })
}

use crate::backends::{CompiledServiceTarget, L4ResolveError, SelectedL4Backend};
use crate::plugins::stream::CompiledStreamPlugins;
use crate::plugins::{StreamPluginResult, TlsRouteContext};
use crate::routes::common::{l4_post_select, L4PostSelect};
use crate::routes::l4_duplex::{DuplexError, DuplexHooks};
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
pub(super) fn apply_tls_l4_post_select(ctx: &mut TlsContext, selected: SelectedL4Backend<'_>) -> bool {
    match l4_post_select(selected) {
        L4PostSelect::FailClosed { status, log } => {
            ctx.status = Some(status);
            ctx.err_log = Some(log.to_string());
            true
        }
        L4PostSelect::Proceed => false,
    }
}

/// One real selection/lookup attempt shared by both proxy loops.
/// Terminal results must return from the connection, never reselect a sibling.
pub(super) enum TlsBackendAttempt<'a, T> {
    Ready(&'a CompiledServiceTarget, T),
    Retryable(&'a CompiledServiceTarget),
    Terminal,
}

/// One attempt for a Service backend, including the async per-connection DNS
/// step (`gateway`-mode first answer, `system`-mode OS lookup).
pub(super) async fn resolve_tls_service_attempt<'a>(
    unit: &'a TlsRouteUnit,
    ctx: &mut TlsContext,
    gateway_key: Option<&str>,
    gateway_config: &crate::config::ResolvedGatewayConfig,
) -> TlsBackendAttempt<'a, Arc<crate::lb::LbBackend>> {
    let Some(selected) = select_tls_backend(unit, ctx) else {
        return TlsBackendAttempt::Terminal;
    };
    // Earlier attempts of this connection (system-mode retries prefer
    // addresses not dialed yet).
    let dialed: Vec<std::net::IpAddr> = ctx
        .upstream_info
        .iter()
        .filter_map(|info| info.addr.as_deref()?.parse().ok())
        .collect();
    // One first-answer wait per connection, on whichever attempt first meets
    // a pending target, so the wait stays bounded per connection.
    let transport = if ctx.dns_first_answer_waited {
        crate::backends::L4Transport::ConnectionRetry
    } else {
        ctx.dns_first_answer_waited = selected
            .service()
            .is_ok_and(|target| crate::backends::dns_endpoint_table().is_pending(target.key()));
        crate::backends::L4Transport::Connection
    };
    let prep = crate::backends::prepare_l4_service(selected, gateway_key, gateway_config, transport, &dialed).await;
    if prep.is_failed() {
        // A DNS failure is not retried with the same lookup: end the connection.
        ctx.err_log = Some(crate::routes::common::L4_DNS_RESOLUTION_FAILED_LOG.to_owned());
        return TlsBackendAttempt::Terminal;
    }
    resolve_selected_tls_backend(selected, ctx, gateway_key, |target| prep.resolve(target))
}

fn select_tls_backend<'a>(unit: &'a TlsRouteUnit, ctx: &mut TlsContext) -> Option<SelectedL4Backend<'a>> {
    let selected = unit.select_backend().ok();
    if selected.is_none() {
        ctx.err_log = Some("no backend".to_string());
    }
    selected
}

fn resolve_selected_tls_backend<'a, T, E>(
    selected: SelectedL4Backend<'a>,
    ctx: &mut TlsContext,
    gateway_key: Option<&str>,
    resolve: impl FnOnce(&'a CompiledServiceTarget) -> Result<T, E>,
) -> TlsBackendAttempt<'a, T> {
    match crate::backends::policy::resolve_l4_policy_backend(selected, gateway_key, |target| {
        resolve(target).map_err(|_| target)
    }) {
        Ok((target, endpoint)) => TlsBackendAttempt::Ready(target, endpoint),
        Err(L4ResolveError::Unavailable(target)) => TlsBackendAttempt::Retryable(target),
        Err(L4ResolveError::Rejected(crate::backends::L4BackendRejection::Denied)) => {
            ctx.status = Some(crate::routes::common::L4_BACKEND_REF_DENIED_STATUS);
            ctx.err_log = Some(crate::routes::common::L4_BACKEND_REF_DENIED_LOG.to_owned());
            TlsBackendAttempt::Terminal
        }
        Err(L4ResolveError::Rejected(_)) => {
            apply_tls_l4_post_select(ctx, selected);
            TlsBackendAttempt::Terminal
        }
    }
}

pub(super) fn tls_connect_retry_delay(attempt: u32, max_attempts: u32) -> Option<Duration> {
    (attempt.saturating_add(1) < max_attempts).then(|| tls_connect_retry_backoff(attempt))
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
    resource: Option<&CompiledStreamPlugins>,
    ctx: &mut TlsContext,
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

    let Some(resource) = resource else {
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
                sni: ctx.sni.as_deref().unwrap_or_default(),
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
#[path = "../../../tests/unit/routes/tls/mod_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/unit/routes/tls/mod_test/sensitive_log_tests.rs"]
mod sensitive_log_tests;
