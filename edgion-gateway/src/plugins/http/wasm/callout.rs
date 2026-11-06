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
use crate::plugins::http::common::outbound::egress::{
    default_block_private, egress_dns_cache_ttl, EgressPolicy, EgressResolver,
};
use crate::plugins::http::common::outbound::request::sync_outbound_body_framing;
use crate::util::http_body::stream_body_capped;
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
/// client is enough: resolved answers live in the bounded process-wide egress
/// answer cache, and classification runs on every use.
/// Callouts run on the request hot path, so blocked addresses emit only the
/// bounded egress metric; per-address tracing is intentionally disabled.
fn callout_client() -> Result<&'static reqwest::Client, &'static str> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
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
                total_timeout: None, // the outer callout deadline governs
                redirect: OutboundRedirect::Deny,
                disable_proxy: true,
                dns_resolver: Some(egress),
                ..Default::default()
            })
            .map_err(|e| format!("wasm callout client unavailable: {e}"))
        })
        .as_ref()
        .map_err(String::as_str)
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
    request_deadline: Option<tokio::time::Instant>,
) -> Result<CalloutOutcome, String> {
    const MAX_ROUNDS: usize = 4;
    let mut delivered = 0usize;
    let mut abandoned = 0usize;
    for _round in 0..MAX_ROUNDS {
        let pending = std::mem::take(&mut lease.state_mut().frame.pending_callouts);
        if pending.is_empty() {
            break;
        }
        let mut pending = pending.into_iter();
        while let Some(callout) = pending.next() {
            if lease.state().frame.continue_requested || lease.state().frame.local_response.is_some() {
                // The SDK still owns registrations for these undelivered tokens.
                abandoned += 1 + pending.len();
                break;
            }
            if request_deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
                lease.poison();
                return Err("wasm request deadline expired".into());
            }
            let response = execute_callout_with_client_and_permits(
                callout,
                timeout_cap_ms,
                metrics_label,
                callout_client().map_err(|_| ()),
                callout_permits(),
                request_deadline,
            )
            .await;
            // An expired request cannot re-enter the guest with a late response.
            if request_deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
                lease.poison();
                return Err("wasm request deadline expired".into());
            }
            let num_headers = response.headers.len() as i32;
            let body_size = response.body.len() as i32;
            let token = response.token;
            lease.state_mut().frame.callout_response = Some(response);
            lease.call_on_http_call_response(ctx_id, token, num_headers, body_size)?;
            lease.state_mut().frame.callout_response = None;
            delivered += 1;
        }
        if lease.state().frame.continue_requested || lease.state().frame.local_response.is_some() {
            break;
        }
    }
    let leftover = abandoned + lease.state_mut().frame.pending_callouts.len();
    lease.state_mut().frame.pending_callouts.clear();
    if leftover != 0 {
        lease.poison();
    }
    Ok(CalloutOutcome { delivered, leftover })
}

/// Execute one callout. Always returns a deliverable [`CalloutResponse`]:
/// on success the headers start with `:status`; on any failure (connect,
/// timeout, oversized body) the headers are empty — the proxy-wasm
/// convention for a failed call, letting the guest pick its own fail
/// behavior instead of trapping.
pub async fn execute_callout(callout: PendingCallout, timeout_cap_ms: u64, metrics_label: &str) -> CalloutResponse {
    execute_callout_with_client_and_permits(
        callout,
        timeout_cap_ms,
        metrics_label,
        callout_client().map_err(|_| ()),
        callout_permits(),
        None,
    )
    .await
}

fn failed_callout_response(token: u32) -> CalloutResponse {
    CalloutResponse {
        token,
        headers: Vec::new(),
        body: Vec::new(),
    }
}

async fn execute_callout_with_client_and_permits(
    callout: PendingCallout,
    timeout_cap_ms: u64,
    metrics_label: &str,
    client: Result<&reqwest::Client, ()>,
    permits: &tokio::sync::Semaphore,
    request_deadline: Option<tokio::time::Instant>,
) -> CalloutResponse {
    let failure = failed_callout_response(callout.token);
    let timeout_ms = match callout.timeout_ms as u64 {
        0 => timeout_cap_ms,
        t => t.min(timeout_cap_ms),
    };
    let now = tokio::time::Instant::now();
    let deadline = now + Duration::from_millis(timeout_ms.max(1));
    let deadline = request_deadline.map_or(deadline, |request| request.min(deadline));
    if deadline <= now {
        record_wasm_callout(metrics_label, "error");
        return failure;
    }

    let attempt = async {
        let _permit = permits.acquire().await.map_err(|_| ())?;
        // A permit can become ready at the timeout boundary. Recheck before
        // constructing/sending the request even if timeout_at polls us first.
        if tokio::time::Instant::now() >= deadline {
            return Err(());
        }
        let client = client?;
        execute_callout_attempt(callout, client).await
    };

    match tokio::time::timeout_at(deadline, attempt).await {
        Ok(Ok(response)) => {
            record_wasm_callout(metrics_label, "ok");
            response
        }
        Ok(Err(())) | Err(_) => {
            record_wasm_callout(metrics_label, "error");
            failure
        }
    }
}

fn build_callout_request(callout: PendingCallout, client: &reqwest::Client) -> Result<(u32, reqwest::Request), ()> {
    let token = callout.token;
    let url = format!("{}{}", callout.origin, callout.path);
    // Request hot path: the guest-supplied method / URL (query may carry
    // credentials) must not reach tracing.
    let method = reqwest::Method::from_bytes(callout.method.as_bytes()).map_err(|_| ())?;

    let mut req = client.request(method, &url);
    if let Some(authority) = callout.authority {
        req = req.header(reqwest::header::HOST, authority);
    }
    for (name, value) in callout.headers {
        if let (Ok(n), Ok(v)) = (
            reqwest::header::HeaderName::from_bytes(name.as_bytes()),
            reqwest::header::HeaderValue::try_from(value),
        ) {
            req = req.header(n, v);
        }
    }
    if !callout.body.is_empty() {
        req = req.body(callout.body);
    }

    let mut req = req.build().map_err(|_| ())?;
    sync_outbound_body_framing(&mut req).map_err(|_| ())?;
    Ok((token, req))
}

/// Check before copying. Lossy UTF-8 can expand values at most threefold,
/// so payload copies stay below 192 KiB plus the fixed status and 1,025 pairs.
fn validate_response_headers(headers: &reqwest::header::HeaderMap) -> Result<(), ()> {
    if headers.len() > 1024 {
        return Err(());
    }
    let total = headers.iter().try_fold(0usize, |total, (name, value)| {
        total
            .checked_add(name.as_str().len())?
            .checked_add(value.len())
            .filter(|total| *total <= 64 * 1024)
    });
    if total.is_none_or(|total| total > 64 * 1024) {
        return Err(());
    }
    Ok(())
}

async fn execute_callout_attempt(callout: PendingCallout, client: &reqwest::Client) -> Result<CalloutResponse, ()> {
    let (token, req) = build_callout_request(callout, client)?;
    let resp = client.execute(req).await.map_err(|_| ())?;

    let status = resp.status().as_u16();
    validate_response_headers(resp.headers())?;
    let mut headers = Vec::with_capacity(resp.headers().len() + 1);
    headers.push((":status".to_string(), status.to_string()));
    for (name, value) in resp.headers() {
        headers.push((
            name.as_str().to_string(),
            String::from_utf8_lossy(value.as_bytes()).into_owned(),
        ));
    }

    let (body, truncated) = stream_body_capped(resp, CALLOUT_MAX_RESPONSE_BYTES)
        .await
        .map_err(|_| ())?;
    if truncated {
        return Err(());
    }

    Ok(CalloutResponse {
        token,
        headers,
        body: body.into(),
    })
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/wasm/callout_test.rs"]
mod tests;
