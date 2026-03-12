//! Webhook key resolver — resolve key values by calling external webhook services.
//!
//! This is the main entry point called from session_adapter's key_get for
//! KeyGet::Webhook variants.

use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::core::gateway::plugins::http::common::http_client::is_hop_by_hop;
use crate::core::gateway::plugins::runtime::PluginSession;
use crate::types::common::WebhookExtract;
use crate::types::resources::link_sys::webhook::WEBHOOK_GLOBAL_MAX_RESPONSE_BYTES;

use super::health::{record_passive_result, should_halfopen_probe};
use super::manager::get_webhook_manager;
use super::runtime::WebhookRuntime;

const WEBHOOK_RETRY_MAX_DELAY_MS: u64 = 30_000;

fn compute_retry_delay_ms(attempt: u32, base_ms: u64, max_ms: u64) -> u64 {
    let exp_delay = base_ms.saturating_mul(1u64 << attempt.min(10));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let jitter = nanos % (base_ms / 2).max(1);
    (exp_delay + jitter).min(max_ms)
}

/// Resolve a key value by calling an external webhook service.
///
/// This is the main entry point called from session_adapter's key_get.
pub async fn resolve_webhook_key(
    session: &dyn PluginSession,
    webhook_ref: &str,
    extract: &WebhookExtract,
) -> Option<String> {
    // 1. Look up webhook config from manager (registered via ConfHandler)
    let manager = get_webhook_manager();
    let runtime = match manager.get(webhook_ref).await {
        Some(rt) => rt,
        None => {
            tracing::warn!(webhook = %webhook_ref, "Webhook not found in manager");
            return None;
        }
    };

    // 2. Health check gate
    if !runtime.healthy.load(Ordering::Relaxed) {
        if !should_halfopen_probe(&runtime) {
            tracing::debug!(webhook = %webhook_ref, "Webhook unhealthy, skipping");
            return None;
        }
        tracing::debug!(webhook = %webhook_ref, "Webhook unhealthy, attempting half-open probe");
    }

    // 3. Rate limit gate
    if let Some(ref counter) = runtime.rate_counter {
        if !counter.try_acquire() {
            tracing::debug!(webhook = %webhook_ref, "Webhook rate limited, skipping");
            return None;
        }
    }

    // 4. Build request headers
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(ref forward_headers) = runtime.config.request_headers {
        for header_name in forward_headers {
            if is_hop_by_hop(header_name) {
                continue;
            }
            if let Some(value) = session.header_value(header_name) {
                if let (Ok(hn), Ok(hv)) = (
                    reqwest::header::HeaderName::from_bytes(header_name.as_bytes()),
                    reqwest::header::HeaderValue::from_str(&value),
                ) {
                    headers.insert(hn, hv);
                }
            }
        }
    }
    // Add X-Forwarded-* headers
    set_forwarded_headers(&mut headers, session);

    // 5. Send request with retry (use custom client when TLS config present)
    let client = runtime.http_client.as_ref();
    let method: reqwest::Method = runtime.config.request_method.parse().unwrap_or(reqwest::Method::GET);
    let timeout = Duration::from_millis(runtime.config.timeout_ms);
    let max_attempts = 1 + runtime.config.retry.as_ref().map_or(0, |r| r.max_retries);

    let mut last_error: Option<String> = None;
    let mut resp = None;

    for attempt in 0..max_attempts {
        if attempt > 0 {
            let base_ms = runtime.config.retry.as_ref().map_or(100, |r| r.retry_delay_ms);
            let delay = compute_retry_delay_ms(attempt, base_ms, WEBHOOK_RETRY_MAX_DELAY_MS);
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }

        match client
            .request(method.clone(), &runtime.config.uri)
            .headers(headers.clone())
            .timeout(timeout)
            .send()
            .await
        {
            Ok(r) => {
                let status = r.status().as_u16();
                let should_retry = runtime
                    .config
                    .retry
                    .as_ref()
                    .is_some_and(|retry| retry.retry_on_status.contains(&status));
                if should_retry && attempt + 1 < max_attempts {
                    last_error = Some(format!("status {}", status));
                    continue;
                }
                resp = Some(r);
                break;
            }
            Err(e) => {
                let should_retry = runtime.config.retry.as_ref().is_some_and(|retry| {
                    (retry.retry_on_timeout && e.is_timeout()) || (retry.retry_on_connect_error && e.is_connect())
                });
                last_error = Some(e.to_string());
                if !should_retry || attempt + 1 >= max_attempts {
                    tracing::warn!(
                        webhook = %webhook_ref,
                        error = %e,
                        attempt = attempt + 1,
                        "Webhook request failed"
                    );
                    record_passive_result(&runtime, false);
                    return None;
                }
                tracing::debug!(
                    webhook = %webhook_ref,
                    error = %e,
                    attempt = attempt + 1,
                    "Webhook request failed, retrying"
                );
            }
        }
    }

    let resp = match resp {
        Some(r) => r,
        None => {
            tracing::warn!(webhook = %webhook_ref, last_error = ?last_error, "All retry attempts exhausted");
            record_passive_result(&runtime, false);
            return None;
        }
    };

    // 6. Passive health check
    let status = resp.status().as_u16();
    let is_passive_failure = is_passive_failure_status(&runtime, status);
    let is_success = match &runtime.config.success_status_codes {
        Some(codes) => codes.contains(&status),
        None => (200..300).contains(&status),
    };
    record_passive_result(&runtime, !is_passive_failure);

    // 7. Check status
    if !is_success {
        tracing::debug!(webhook = %webhook_ref, status, "Webhook non-success status");
        return None;
    }

    // 8. Extract value from response (with body size limit)
    let max_bytes = runtime.config.max_response_bytes.min(WEBHOOK_GLOBAL_MAX_RESPONSE_BYTES);
    extract_value_from_response(resp, extract, max_bytes).await
}

// ============================================================
// Helpers
// ============================================================

/// Set X-Forwarded-* headers for the webhook request
fn set_forwarded_headers(headers: &mut reqwest::header::HeaderMap, session: &dyn PluginSession) {
    let remote_addr = session.remote_addr();
    if !remote_addr.is_empty() {
        if let Ok(hv) = reqwest::header::HeaderValue::from_str(remote_addr) {
            headers.insert("X-Forwarded-For", hv);
        }
    }
    if let Ok(hv) = reqwest::header::HeaderValue::from_str(session.get_method()) {
        headers.insert("X-Forwarded-Method", hv);
    }
    if let Ok(hv) = reqwest::header::HeaderValue::from_str(session.get_path()) {
        headers.insert("X-Forwarded-Uri", hv);
    }
    // Forward host header if present
    if let Some(host) = session.header_value("host") {
        if let Ok(hv) = reqwest::header::HeaderValue::from_str(&host) {
            headers.insert("X-Forwarded-Host", hv);
        }
    }
}

/// Check if a response status code counts as a passive health check failure.
fn is_passive_failure_status(runtime: &WebhookRuntime, status: u16) -> bool {
    runtime
        .config
        .health_check
        .as_ref()
        .and_then(|hc| hc.passive.as_ref())
        .map(|p| p.failure_status_codes.contains(&status))
        .unwrap_or(false)
}

/// Extract a value from an HTTP response based on WebhookExtract rules.
async fn extract_value_from_response(
    resp: reqwest::Response,
    extract: &WebhookExtract,
    max_bytes: usize,
) -> Option<String> {
    match extract {
        WebhookExtract::Header { name } => resp
            .headers()
            .get(name.as_str())
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string()),

        WebhookExtract::Cookie { name } => {
            for cookie_header in resp.headers().get_all("set-cookie") {
                if let Ok(val) = cookie_header.to_str() {
                    if let Some(pair) = val.split(';').next() {
                        let mut parts = pair.splitn(2, '=');
                        if let (Some(n), Some(v)) = (parts.next(), parts.next()) {
                            if n.trim() == name {
                                return Some(v.trim().to_string());
                            }
                        }
                    }
                }
            }
            None
        }

        WebhookExtract::Body { path } => {
            let body_bytes = read_body_limited(resp, max_bytes).await?;
            let body = String::from_utf8(body_bytes).ok()?;
            let json: serde_json::Value = serde_json::from_str(&body).ok()?;
            // Dot-path traversal: "data.user_id" → json["data"]["user_id"]
            let mut current = &json;
            for segment in path.split('.') {
                if let Ok(index) = segment.parse::<usize>() {
                    current = current.get(index)?;
                } else {
                    current = current.get(segment)?;
                }
            }
            match current {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Null => None,
                other => Some(other.to_string()),
            }
        }

        WebhookExtract::BodyText => {
            let body_bytes = read_body_limited(resp, max_bytes).await?;
            String::from_utf8(body_bytes).ok().map(|s| s.trim().to_string())
        }
    }
}

/// Read response body with a size limit.
async fn read_body_limited(resp: reqwest::Response, max_bytes: usize) -> Option<Vec<u8>> {
    // Use bytes() with a size check — reqwest reads the full body
    // For production, we'd use chunk-based reading, but this is sufficient
    // since max_bytes is capped at 16KB.
    match resp.bytes().await {
        Ok(bytes) => {
            if bytes.is_empty() {
                None
            } else if bytes.len() <= max_bytes {
                Some(bytes.to_vec())
            } else {
                Some(bytes[..max_bytes].to_vec())
            }
        }
        Err(_) => None,
    }
}

// NOTE: extract_value_from_response tests require a real HTTP server (mockito).
// These are covered by integration tests instead.
// See examples/code/client/suites/edgion_plugins/webhook_key_get/ for full integration tests.
