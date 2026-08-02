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
    execute_callout_with_client_and_permits(
        callout,
        timeout_cap_ms,
        metrics_label,
        callout_client().map_err(|_| ()),
        callout_permits(),
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
    callout: &PendingCallout,
    timeout_cap_ms: u64,
    metrics_label: &str,
    client: Result<&reqwest::Client, ()>,
    permits: &tokio::sync::Semaphore,
) -> CalloutResponse {
    let failure = failed_callout_response(callout.token);
    let timeout_ms = match callout.timeout_ms as u64 {
        0 => timeout_cap_ms,
        t => t.min(timeout_cap_ms),
    };
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms.max(1));

    let attempt = async {
        let _permit = permits.acquire().await.map_err(|_| ())?;
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

async fn execute_callout_attempt(callout: &PendingCallout, client: &reqwest::Client) -> Result<CalloutResponse, ()> {
    let url = format!("{}{}", callout.origin, callout.path);
    // Request hot path: the guest-supplied method / URL (query may carry
    // credentials) must not reach tracing.
    let method = reqwest::Method::from_bytes(callout.method.as_bytes()).map_err(|_| ())?;

    let mut req = client.request(method, &url);
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

    let resp = req.send().await.map_err(|_| ())?;

    let status = resp.status().as_u16();
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
        token: callout.token,
        headers,
        body: body.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    fn callout(origin: String, timeout_ms: u32) -> PendingCallout {
        PendingCallout {
            token: 7,
            origin,
            path: "/callout".to_string(),
            method: "GET".to_string(),
            authority: None,
            headers: Vec::new(),
            body: Vec::new(),
            timeout_ms,
        }
    }

    fn test_client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().expect("test client")
    }

    #[tokio::test]
    async fn unavailable_guarded_client_fails_without_network() {
        let server = wiremock::MockServer::start().await;
        let callout = callout(server.uri(), 1_000);
        let permits = tokio::sync::Semaphore::new(1);

        let response = execute_callout_with_client_and_permits(&callout, 1_000, "test", Err(()), &permits).await;

        assert_eq!(response.token, callout.token);
        assert!(response.headers.is_empty());
        assert!(response.body.is_empty());
        assert!(
            server.received_requests().await.expect("read mock requests").is_empty(),
            "client initialization failure must not fall back to an unguarded request"
        );
    }

    #[tokio::test]
    async fn zero_permit_semaphore_is_bounded_by_callout_deadline() {
        let client = test_client();
        let permits = tokio::sync::Semaphore::new(0);
        let callout = callout("http://127.0.0.1:9".to_string(), 0);

        let response = tokio::time::timeout(
            Duration::from_secs(2),
            execute_callout_with_client_and_permits(&callout, 25, "test", Ok(&client), &permits),
        )
        .await
        .expect("callout deadline must bound semaphore wait");

        assert!(response.headers.is_empty());
        assert!(response.body.is_empty());
    }

    #[tokio::test]
    async fn held_permit_until_deadline_prevents_network_request() {
        let server = wiremock::MockServer::start().await;
        let client = test_client();
        let permits = tokio::sync::Semaphore::new(1);
        let held = permits.acquire().await.expect("hold only permit");
        let callout = callout(server.uri(), 30);

        let response = tokio::time::timeout(
            Duration::from_secs(2),
            execute_callout_with_client_and_permits(&callout, 1_000, "test", Ok(&client), &permits),
        )
        .await
        .expect("callout deadline must expire");

        assert!(response.headers.is_empty());
        assert!(response.body.is_empty());
        assert!(server.received_requests().await.expect("read mock requests").is_empty());
        drop(held);
    }

    #[tokio::test]
    async fn permit_released_before_deadline_allows_real_callout() {
        let server = wiremock::MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/callout"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ok".to_vec()))
            .mount(&server)
            .await;
        let client = test_client();
        let permits = tokio::sync::Semaphore::new(1);
        let held = permits.acquire().await.expect("hold only permit");
        let callout = callout(server.uri(), 1_000);

        let run_callout = execute_callout_with_client_and_permits(&callout, 1_000, "test", Ok(&client), &permits);
        let release = async move {
            tokio::task::yield_now().await;
            drop(held);
        };
        let (response, ()) = tokio::time::timeout(Duration::from_secs(2), async { tokio::join!(run_callout, release) })
            .await
            .expect("released permit must unblock callout");

        assert_eq!(response.headers.first().map(|h| h.1.as_str()), Some("200"));
        assert_eq!(response.body, b"ok");
    }

    #[tokio::test]
    async fn body_stream_timeout_drops_in_flight_permit_without_leak() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("test server address");
        let (headers_sent_tx, headers_sent_rx) = tokio::sync::oneshot::channel();
        let server_task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept callout");
            let mut request = [0u8; 1024];
            let bytes_read = stream.read(&mut request).await.expect("read callout");
            assert!(bytes_read > 0, "callout request must not be empty");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n")
                .await
                .expect("write response headers");
            let _ = headers_sent_tx.send(());
            std::future::pending::<()>().await;
        });

        let client = std::sync::Arc::new(test_client());
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let callout = callout(format!("http://{addr}"), 500);
        let call_client = client.clone();
        let call_permits = permits.clone();
        let call_task = tokio::spawn(async move {
            execute_callout_with_client_and_permits(&callout, 1_000, "test", Ok(&call_client), &call_permits).await
        });

        tokio::time::timeout(Duration::from_secs(2), headers_sent_rx)
            .await
            .expect("callout must receive response headers before its deadline")
            .expect("test server must report sent headers");
        let response = tokio::time::timeout(Duration::from_secs(2), call_task)
            .await
            .expect("body stream must obey callout deadline")
            .expect("callout task must not panic");

        assert!(response.headers.is_empty());
        assert!(response.body.is_empty());
        let reacquired = tokio::time::timeout(Duration::from_secs(1), permits.acquire())
            .await
            .expect("timed-out callout must release permit")
            .expect("semaphore remains open");
        drop(reacquired);
        server_task.abort();
    }
}
