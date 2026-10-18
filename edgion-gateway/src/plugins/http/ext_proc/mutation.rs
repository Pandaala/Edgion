//! Translation between the gateway session and the ext_proc wire types.
//!
//! Two directions:
//! - **collect**: snapshot request/response headers into a proto `HeaderMap`
//!   (including the read-only pseudo-headers Envoy processors expect).
//! - **apply**: fold a processor-returned `HeaderMutation` back onto the
//!   session, honoring `append_action` and refusing to touch protected headers —
//!   HTTP/2 pseudo-headers (task pitfall #7: no rewriting `:method` etc.) and
//!   proxy-managed hop-by-hop/framing headers (smuggling guard).

use edgion_common::ext_proc::proto::{HeaderAppendAction, HeaderMap, HeaderMutation, HeaderValue, HeaderValueOption};

use crate::plugins::runtime::{PluginLog, PluginSession};
use crate::PendingHeaderOp;

/// Whether `name` is an HTTP/2 pseudo-header (leading `:`). These are sent to
/// the processor read-only and are never mutated based on its response.
pub fn is_pseudo_header(name: &str) -> bool {
    name.starts_with(':')
}

/// Hop-by-hop and message-framing headers owned by the proxy. An external
/// processor must never set/append/remove these: doing so enables request/
/// response smuggling (`Content-Length` / `Transfer-Encoding` desync) and
/// connection-pool corruption (`Connection` / `Keep-Alive` / `Upgrade`).
/// Mirrors Envoy ext_proc's default `disallow_system_headers` posture.
fn is_request_proxy_managed_header(name: &str) -> bool {
    const MANAGED: [&str; 8] = [
        "connection",
        "keep-alive",
        "proxy-connection",
        "transfer-encoding",
        "content-length",
        "te",
        "upgrade",
        "trailer",
    ];
    MANAGED.iter().any(|m| name.eq_ignore_ascii_case(m))
}

pub(super) fn is_protected_header(name: &str) -> bool {
    edgion_resources::response_header_policy::is_protected_response_header(name)
}

/// Decode a processor-supplied header value, preferring the modern `raw_value`
/// (bytes) field and falling back to the legacy `value` (string) field.
///
/// Empty values require `keep_empty_value`. Invalid HTTP value bytes are logged
/// and skipped without exposing or rewriting processor-supplied contents.
fn decoded_value(opt: &HeaderValueOption, hv: &HeaderValue, log: &mut PluginLog) -> Option<http::HeaderValue> {
    let raw = if hv.raw_value.is_empty() {
        hv.value.as_bytes()
    } else {
        &hv.raw_value
    };
    if raw.is_empty() && !opt.keep_empty_value {
        return None;
    }
    match http::HeaderValue::from_bytes(raw) {
        Ok(value) => Some(value),
        Err(_) => {
            log.push_err("invalid header value");
            None
        }
    }
}

/// Build a proto `HeaderMap` from the request headers plus read-only pseudo
/// headers (`:method`, `:path`, `:authority`).
/// `:path` includes the current normalized path and raw current query string.
pub fn collect_request_headers(session: &dyn PluginSession) -> HeaderMap {
    let method = session.get_method().to_string();
    let uri = session.get_uri();
    let request = session.request_header_map();
    let authority = request.get("host");
    let mut headers = Vec::with_capacity(request.len() + 2 + usize::from(authority.is_some()));
    push_pseudo(&mut headers, ":method", method);
    push_pseudo(&mut headers, ":path", uri);
    if let Some(authority) = authority {
        headers.push(raw_header(":authority".to_string(), authority.as_bytes().to_vec()));
    }
    for (name, value) in request {
        headers.push(raw_header(name.as_str().to_string(), value.as_bytes().to_vec()));
    }
    HeaderMap { headers }
}

/// Build a proto `HeaderMap` from the response headers plus the read-only
/// `:status` pseudo header. Uses the all-pairs accessor so multi-valued headers
/// (e.g. `Set-Cookie`) are preserved rather than collapsed to the first value.
pub fn collect_response_headers(session: &dyn PluginSession) -> HeaderMap {
    let mut headers = Vec::new();
    if let Some(status) = session.get_response_status() {
        push_pseudo(&mut headers, ":status", status.to_string());
    }
    for (name, value) in session.response_headers_bytes() {
        // Canonical adapters expose ASCII HeaderNames; do not repair bytes from
        // another adapter using a lossy conversion.
        if let Ok(name) = String::from_utf8(name) {
            headers.push(raw_header(name, value));
        }
    }
    HeaderMap { headers }
}

fn push_pseudo(headers: &mut Vec<HeaderValue>, key: &str, value: String) {
    headers.push(raw_header(key.to_string(), value.into_bytes()));
}

fn raw_header(key: String, raw_value: Vec<u8>) -> HeaderValue {
    HeaderValue {
        key,
        value: String::new(),
        raw_value,
    }
}

/// Apply a processor `HeaderMutation` to the upstream-bound **request** headers.
/// Returns the number of headers actually changed (for the access log; no-op
/// conditional actions are not counted).
pub fn apply_to_request(session: &mut dyn PluginSession, mutation: &HeaderMutation, log: &mut PluginLog) -> usize {
    let mut applied = 0;
    for opt in &mutation.set_headers {
        let Some(hv) = opt.header.as_ref() else { continue };
        if reject_protected_request(&hv.key, log) {
            continue;
        }
        let Some(value) = decoded_value(opt, hv, log) else {
            continue;
        };
        let action = HeaderAppendAction::try_from(opt.append_action).unwrap_or(HeaderAppendAction::AppendIfExistsOrAdd);
        // `Some(result)` means a mutation was attempted; `None` is a conditional no-op.
        let outcome = match action {
            HeaderAppendAction::AppendIfExistsOrAdd => Some(session.append_request_header_value(&hv.key, value)),
            HeaderAppendAction::OverwriteIfExistsOrAdd => Some(session.set_request_header_value(&hv.key, value)),
            HeaderAppendAction::AddIfAbsent => (!session.request_header_map().contains_key(&hv.key))
                .then(|| session.set_request_header_value(&hv.key, value)),
            HeaderAppendAction::OverwriteIfExists => (session.request_header_map().contains_key(&hv.key))
                .then(|| session.set_request_header_value(&hv.key, value)),
        };
        match outcome {
            Some(Ok(())) => applied += 1,
            Some(Err(_)) => {
                log.push_err("req header set fail");
            }
            None => {}
        }
    }
    for name in &mutation.remove_headers {
        if reject_protected_request(name, log) {
            continue;
        }
        if session.remove_request_header(name).is_ok() {
            applied += 1;
        }
    }
    applied
}

/// Apply a processor `HeaderMutation` to the **response** headers.
/// Returns the number of headers actually changed (for the access log; no-op
/// conditional actions are not counted).
pub fn apply_to_response(session: &mut dyn PluginSession, mutation: &HeaderMutation, log: &mut PluginLog) -> usize {
    let mut applied = 0;
    for opt in &mutation.set_headers {
        let Some(hv) = opt.header.as_ref() else { continue };
        if reject_protected(&hv.key, log) {
            continue;
        }
        let Some(value) = decoded_value(opt, hv, log) else {
            continue;
        };
        let action = HeaderAppendAction::try_from(opt.append_action).unwrap_or(HeaderAppendAction::AppendIfExistsOrAdd);
        let outcome = match action {
            HeaderAppendAction::AppendIfExistsOrAdd => Some(session.append_response_header_value(&hv.key, value)),
            HeaderAppendAction::OverwriteIfExistsOrAdd => Some(session.set_response_header_value(&hv.key, value)),
            HeaderAppendAction::AddIfAbsent => {
                (!session.has_response_header(&hv.key)).then(|| session.set_response_header_value(&hv.key, value))
            }
            HeaderAppendAction::OverwriteIfExists => {
                (session.has_response_header(&hv.key)).then(|| session.set_response_header_value(&hv.key, value))
            }
        };
        match outcome {
            Some(Ok(())) => applied += 1,
            Some(Err(_)) => {
                log.push_err("resp header set fail");
            }
            None => {}
        }
    }
    for name in &mutation.remove_headers {
        if reject_protected(name, log) {
            continue;
        }
        if session.remove_response_header(name).is_ok() {
            applied += 1;
        }
    }
    applied
}

/// Translate an ExtProc mutation into generic operations for a fresh local
/// reply. Conditions are deliberately evaluated when the fresh header exists,
/// rather than against an upstream response that the local reply replaces.
pub fn queue_for_local_reply(session: &mut dyn PluginSession, mutation: HeaderMutation, log: &mut PluginLog) -> usize {
    let ops = local_reply_ops(mutation, log);
    let mut queued = 0;
    for op in ops {
        if session.queue_local_reply_header_op(op).is_ok() {
            queued += 1;
        } else {
            log.push_err("resp header set fail");
        }
    }
    queued
}

/// Convert ExtProc wire mutations into runtime-owned local-reply operations.
/// This is also used by streamed body handlers, which cannot hold a session
/// borrow while awaiting the processor.
pub fn local_reply_ops(mutation: HeaderMutation, log: &mut PluginLog) -> Vec<PendingHeaderOp> {
    let mut ops = Vec::new();
    for mut opt in mutation.set_headers {
        let Some(hv) = opt.header.take() else { continue };
        if reject_protected(&hv.key, log) {
            continue;
        }
        let Some(value) = decoded_value(&opt, &hv, log) else {
            continue;
        };
        let action = HeaderAppendAction::try_from(opt.append_action).unwrap_or(HeaderAppendAction::AppendIfExistsOrAdd);
        let op = match action {
            HeaderAppendAction::AppendIfExistsOrAdd => PendingHeaderOp::Append { name: hv.key, value },
            HeaderAppendAction::OverwriteIfExistsOrAdd => PendingHeaderOp::Set { name: hv.key, value },
            HeaderAppendAction::AddIfAbsent => PendingHeaderOp::AddIfAbsent { name: hv.key, value },
            HeaderAppendAction::OverwriteIfExists => PendingHeaderOp::OverwriteIfExists { name: hv.key, value },
        };
        ops.push(op);
    }
    for name in mutation.remove_headers {
        if reject_protected(&name, log) {
            continue;
        }
        ops.push(PendingHeaderOp::Remove { name });
    }
    ops
}

/// Log-and-skip helper: returns true if the processor must not mutate `name` —
/// either an HTTP/2 pseudo-header or a proxy-managed hop-by-hop/framing header.
pub(super) fn reject_protected(name: &str, log: &mut PluginLog) -> bool {
    if is_pseudo_header(name) {
        log.push_err("pseudo header mutation rejected");
        true
    } else if is_protected_header(name) {
        log.push_err("protected header mutation rejected");
        true
    } else {
        false
    }
}

/// Request mutation keeps its existing policy; response policy must not tighten it.
fn reject_protected_request(name: &str, log: &mut PluginLog) -> bool {
    if is_pseudo_header(name) {
        log.push_err("pseudo header mutation rejected");
        true
    } else if is_request_proxy_managed_header(name) {
        log.push_err("protected header mutation rejected");
        true
    } else {
        false
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/ext_proc/mutation_test.rs"]
mod tests;
