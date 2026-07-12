//! Guest-dispatched HTTP callout execution.
//!
//! `proxy_dispatch_http_call` queues a [`PendingCallout`] in `HostState`;
//! an async host path ([`resolve_callouts_on`]) calls [`execute_callout`] for
//! each queued entry, then delivers the result back through
//! `proxy_on_http_call_response`. Two callers exist: the plugin's request
//! path (guest paused on an open per-request context) and the module tick
//! task (root context). Targets are already allowlist-checked at dispatch
//! time (see `host_abi`).

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use super::host_abi::{CalloutResponse, PendingCallout};
use super::vm_manager::VmLease;
use crate::link_sys::http_client::{build_outbound_client, OutboundClientConfig, OutboundRedirect};
use crate::observe::metrics::registry::record_wasm_callout;
use crate::plugins::http::common::http_client::get_http_client_arc;
use crate::plugins::http::common::outbound::call::stream_body_capped;
use crate::plugins::http::common::outbound::egress::{
    default_block_private, egress_dns_cache_ttl, EgressPolicy, EgressResolver,
};
use edgion_common::dns::ResolverSpec;

/// Response-body cap for a single callout. A guest-facing verdict payload
/// (auth decision, risk score) is small; anything larger is treated as a
/// failed callout rather than buffered.
const CALLOUT_MAX_RESPONSE_BYTES: usize = 256 * 1024;
/// Process-wide bound on concurrently in-flight callouts, guarding against
/// guest-driven outbound amplification across many paused requests.
const MAX_CONCURRENT_CALLOUTS: usize = 64;

fn callout_permits() -> &'static tokio::sync::Semaphore {
    static PERMITS: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    PERMITS.get_or_init(|| tokio::sync::Semaphore::new(MAX_CONCURRENT_CALLOUTS))
}

/// Shared, egress-guarded client for guest callouts.
///
/// The callout target host is locked to the operator's normalized allowlist
/// origin, but the allowlist is a *string* match and DNS resolution happens
/// per-connect: an allowlisted hostname whose record an attacker can influence
/// (DNS rebinding) would otherwise let an untrusted guest reach loopback /
/// link-local (169.254.169.254) / RFC1918. The `EgressResolver` classifies the
/// exact IPs reqwest connects to — the same guard the webhook/forward_auth
/// path uses — and redirect-Deny prevents a redirect-based escape. One shared
/// client is enough: its address memo is capped (`EGRESS_CACHE_MAX_ENTRIES`).
fn callout_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let egress: Arc<dyn reqwest::dns::Resolve> = Arc::new(EgressResolver::new(
            ResolverSpec::System,
            EgressPolicy {
                block_private: default_block_private(),
            },
            egress_dns_cache_ttl(),
        ));
        build_outbound_client(OutboundClientConfig {
            pool_max_idle_per_host: Some(32),
            connect_timeout: Some(Duration::from_secs(5)),
            total_timeout: None, // per-call `.timeout()` governs
            redirect: OutboundRedirect::Deny,
            dns_resolver: Some(egress),
            ..Default::default()
        })
        // Fall back to the plain shared client only if the guarded build fails
        // (should not happen — no TLS backend to init); never panic the process.
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "wasm callout: egress-guarded client build failed; using shared client");
            (*get_http_client_arc()).clone()
        })
    })
}

/// Delivery counters from one [`resolve_callouts_on`] run; the caller decides
/// how to report them (request path: `PluginLog`; tick path: tracing).
pub struct CalloutOutcome {
    /// Callouts executed and delivered to the guest.
    pub delivered: usize,
    /// Callouts still pending after the round cap (dropped).
    pub leftover: usize,
}

/// Execute the callouts queued in the lease's `HostState` and deliver each
/// result to the guest on `ctx_id` via `proxy_on_http_call_response`.
/// Bounded: the per-phase dispatch cap is enforced at dispatch time, plus a
/// round cap here (a callback may dispatch follow-up callouts). Stops early
/// once the guest resumes or answers locally; anything still pending after
/// the round cap is dropped (counted in [`CalloutOutcome::leftover`]).
pub(super) async fn resolve_callouts_on(
    lease: &mut VmLease,
    ctx_id: i32,
    timeout_cap_ms: u64,
    metrics_label: &str,
) -> Result<CalloutOutcome, String> {
    const MAX_ROUNDS: usize = 4;
    let mut delivered = 0usize;
    for _round in 0..MAX_ROUNDS {
        let pending = std::mem::take(&mut lease.state_mut().pending_callouts);
        if pending.is_empty() {
            break;
        }
        for callout in pending {
            if lease.state().continue_requested || lease.state().local_response.is_some() {
                // The guest already decided; remaining callouts are moot.
                break;
            }
            let response = execute_callout(&callout, timeout_cap_ms, metrics_label).await;
            let num_headers = response.headers.len() as i32;
            let body_size = response.body.len() as i32;
            let token = response.token;
            lease.state_mut().callout_response = Some(response);
            lease.call_on_http_call_response(ctx_id, token, num_headers, body_size)?;
            lease.state_mut().callout_response = None;
            delivered += 1;
        }
        if lease.state().continue_requested || lease.state().local_response.is_some() {
            break;
        }
    }
    let leftover = lease.state_mut().pending_callouts.len();
    lease.state_mut().pending_callouts.clear();
    Ok(CalloutOutcome { delivered, leftover })
}

/// Execute one callout. Always returns a deliverable [`CalloutResponse`]:
/// on success the headers start with `:status`; on any failure (connect,
/// timeout, oversized body) the headers are empty — the proxy-wasm
/// convention for a failed call, letting the guest pick its own fail
/// behavior instead of trapping.
pub async fn execute_callout(callout: &PendingCallout, timeout_cap_ms: u64, metrics_label: &str) -> CalloutResponse {
    let failure = CalloutResponse {
        token: callout.token,
        headers: Vec::new(),
        body: Vec::new(),
    };

    let Ok(_permit) = callout_permits().acquire().await else {
        return failure;
    };

    let timeout_ms = match callout.timeout_ms as u64 {
        0 => timeout_cap_ms,
        t => t.min(timeout_cap_ms),
    };
    let url = format!("{}{}", callout.origin, callout.path);
    let Ok(method) = reqwest::Method::from_bytes(callout.method.as_bytes()) else {
        record_wasm_callout(metrics_label, "error");
        tracing::debug!(method = %callout.method, "wasm callout: invalid method");
        return failure;
    };

    let mut req = callout_client()
        .request(method, &url)
        .timeout(Duration::from_millis(timeout_ms.max(1)));
    if let Some(authority) = &callout.authority {
        req = req.header(reqwest::header::HOST, authority.clone());
    }
    for (name, value) in &callout.headers {
        if let (Ok(n), Ok(v)) = (
            reqwest::header::HeaderName::from_bytes(name.as_bytes()),
            reqwest::header::HeaderValue::from_str(value),
        ) {
            req = req.header(n, v);
        }
    }
    if !callout.body.is_empty() {
        req = req.body(callout.body.clone());
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            record_wasm_callout(metrics_label, "error");
            tracing::debug!(error = %e, %url, "wasm callout failed");
            return failure;
        }
    };

    let status = resp.status().as_u16();
    let mut headers = Vec::with_capacity(resp.headers().len() + 1);
    headers.push((":status".to_string(), status.to_string()));
    for (name, value) in resp.headers() {
        headers.push((
            name.as_str().to_string(),
            String::from_utf8_lossy(value.as_bytes()).into_owned(),
        ));
    }

    let (body, truncated) = match stream_body_capped(resp, CALLOUT_MAX_RESPONSE_BYTES).await {
        Ok(v) => v,
        Err(e) => {
            record_wasm_callout(metrics_label, "error");
            tracing::debug!(error = %e, %url, "wasm callout body read failed");
            return failure;
        }
    };
    if truncated {
        record_wasm_callout(metrics_label, "error");
        tracing::warn!(%url, cap = CALLOUT_MAX_RESPONSE_BYTES, "wasm callout response exceeded size cap");
        return failure;
    }

    record_wasm_callout(metrics_label, "ok");
    CalloutResponse {
        token: callout.token,
        headers,
        body: body.to_vec(),
    }
}
