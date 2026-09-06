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

use crate::plugins::runtime::{PluginLog, PluginSession, PluginSessionHeaderExt};
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
fn is_proxy_managed_header(name: &str) -> bool {
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
    is_pseudo_header(name) || is_proxy_managed_header(name)
}

/// Decode a processor-supplied header value, preferring the modern `raw_value`
/// (bytes) field and falling back to the legacy `value` (string) field.
///
/// Returns `Some("")` only when the option opts in via `keep_empty_value`;
/// otherwise an empty value yields `None` (the mutation is skipped).
fn decoded_value(opt: &HeaderValueOption, hv: &HeaderValue) -> Option<String> {
    if !hv.raw_value.is_empty() {
        // Lossy is acceptable: header values reaching the session API are strings.
        return Some(String::from_utf8_lossy(&hv.raw_value).into_owned());
    }
    if !hv.value.is_empty() {
        return Some(hv.value.clone());
    }
    // Empty value: honor it only when the processor explicitly asked to keep it.
    opt.keep_empty_value.then(String::new)
}

/// Build a proto `HeaderMap` from the request headers plus read-only pseudo
/// headers (`:method`, `:path`, `:authority`).
pub fn collect_request_headers(session: &dyn PluginSession) -> HeaderMap {
    let mut headers = Vec::new();
    push_pseudo(&mut headers, ":method", session.get_method().to_string());
    push_pseudo(&mut headers, ":path", session.get_path().to_string());
    if let Some(authority) = session.header_value("host") {
        push_pseudo(&mut headers, ":authority", authority);
    }
    for (name, value) in session.request_headers() {
        headers.push(string_header(name, value));
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
    for (name, value) in session.response_headers() {
        headers.push(string_header(name, value));
    }
    HeaderMap { headers }
}

fn push_pseudo(headers: &mut Vec<HeaderValue>, key: &str, value: String) {
    headers.push(string_header(key.to_string(), value));
}

fn string_header(key: String, value: String) -> HeaderValue {
    HeaderValue {
        key,
        value,
        raw_value: Vec::new(),
    }
}

/// Apply a processor `HeaderMutation` to the upstream-bound **request** headers.
/// Returns the number of headers actually changed (for the access log; no-op
/// conditional actions are not counted).
pub fn apply_to_request(session: &mut dyn PluginSession, mutation: &HeaderMutation, log: &mut PluginLog) -> usize {
    let mut applied = 0;
    for opt in &mutation.set_headers {
        let Some(hv) = opt.header.as_ref() else { continue };
        if reject_protected(&hv.key, log) {
            continue;
        }
        let Some(value) = decoded_value(opt, hv) else { continue };
        let action = HeaderAppendAction::try_from(opt.append_action).unwrap_or(HeaderAppendAction::AppendIfExistsOrAdd);
        // `Some(result)` means a mutation was attempted; `None` is a conditional no-op.
        let outcome = match action {
            HeaderAppendAction::AppendIfExistsOrAdd => Some(session.append_request_header(&hv.key, &value)),
            HeaderAppendAction::OverwriteIfExistsOrAdd => Some(session.set_request_header(&hv.key, &value)),
            HeaderAppendAction::AddIfAbsent => {
                (session.header_str(&hv.key).is_none()).then(|| session.set_request_header(&hv.key, &value))
            }
            HeaderAppendAction::OverwriteIfExists => {
                (session.header_str(&hv.key).is_some()).then(|| session.set_request_header(&hv.key, &value))
            }
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
        if reject_protected(name, log) {
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
        let Some(value) = decoded_value(opt, hv) else { continue };
        let action = HeaderAppendAction::try_from(opt.append_action).unwrap_or(HeaderAppendAction::AppendIfExistsOrAdd);
        let outcome =
            match action {
                HeaderAppendAction::AppendIfExistsOrAdd => Some(session.append_response_header(&hv.key, &value)),
                HeaderAppendAction::OverwriteIfExistsOrAdd => Some(session.set_response_header(&hv.key, &value)),
                HeaderAppendAction::AddIfAbsent => (session.get_response_header(&hv.key).is_none())
                    .then(|| session.set_response_header(&hv.key, &value)),
                HeaderAppendAction::OverwriteIfExists => (session.get_response_header(&hv.key).is_some())
                    .then(|| session.set_response_header(&hv.key, &value)),
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
pub fn queue_for_local_reply(session: &mut dyn PluginSession, mutation: &HeaderMutation, log: &mut PluginLog) -> usize {
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
pub fn local_reply_ops(mutation: &HeaderMutation, log: &mut PluginLog) -> Vec<PendingHeaderOp> {
    let mut ops = Vec::new();
    for opt in &mutation.set_headers {
        let Some(hv) = opt.header.as_ref() else { continue };
        if reject_protected(&hv.key, log) {
            continue;
        }
        let Some(value) = decoded_value(opt, hv) else { continue };
        let action = HeaderAppendAction::try_from(opt.append_action).unwrap_or(HeaderAppendAction::AppendIfExistsOrAdd);
        let op = match action {
            HeaderAppendAction::AppendIfExistsOrAdd => PendingHeaderOp::Append {
                name: hv.key.clone(),
                value,
            },
            HeaderAppendAction::OverwriteIfExistsOrAdd => PendingHeaderOp::Set {
                name: hv.key.clone(),
                value,
            },
            HeaderAppendAction::AddIfAbsent => PendingHeaderOp::AddIfAbsent {
                name: hv.key.clone(),
                value,
            },
            HeaderAppendAction::OverwriteIfExists => PendingHeaderOp::OverwriteIfExists {
                name: hv.key.clone(),
                value,
            },
        };
        ops.push(op);
    }
    for name in &mutation.remove_headers {
        if reject_protected(name, log) {
            continue;
        }
        ops.push(PendingHeaderOp::Remove { name: name.clone() });
    }
    ops
}

/// Log-and-skip helper: returns true if the processor must not mutate `name` —
/// either an HTTP/2 pseudo-header or a proxy-managed hop-by-hop/framing header.
pub(super) fn reject_protected(name: &str, log: &mut PluginLog) -> bool {
    if is_pseudo_header(name) {
        log.push_err("pseudo header mutation rejected");
        true
    } else if is_proxy_managed_header(name) {
        log.push_err("protected header mutation rejected");
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;

    fn set_header_opt(key: &str, value: &str, action: HeaderAppendAction) -> HeaderValueOption {
        HeaderValueOption {
            header: Some(HeaderValue {
                key: key.to_string(),
                value: value.to_string(),
                raw_value: Vec::new(),
            }),
            append_action: action as i32,
            keep_empty_value: false,
        }
    }

    #[test]
    fn local_reply_ops_preserve_append_conditions_removal_empty_and_duplicates() {
        let mut empty = set_header_opt("x-empty", "", HeaderAppendAction::OverwriteIfExistsOrAdd);
        empty.keep_empty_value = true;
        let mutation = HeaderMutation {
            set_headers: vec![
                set_header_opt("x-duplicate", "one", HeaderAppendAction::AppendIfExistsOrAdd),
                set_header_opt("x-duplicate", "two", HeaderAppendAction::AppendIfExistsOrAdd),
                set_header_opt("x-new", "added", HeaderAppendAction::AddIfAbsent),
                set_header_opt("x-present", "ignored", HeaderAppendAction::AddIfAbsent),
                set_header_opt("x-present", "updated", HeaderAppendAction::OverwriteIfExists),
                set_header_opt("x-missing", "ignored", HeaderAppendAction::OverwriteIfExists),
                empty,
            ],
            remove_headers: vec!["x-remove".to_string()],
        };
        let mut log = PluginLog::new("ExtProc");
        let ops = local_reply_ops(&mutation, &mut log);
        let mut response = pingora_http::ResponseHeader::build(403, Some(8)).expect("valid response");
        response.insert_header("x-present", "old").unwrap();
        response.insert_header("x-remove", "gone").unwrap();
        for op in ops {
            op.apply(&mut response);
        }

        let duplicates = response
            .headers
            .get_all("x-duplicate")
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(duplicates, ["one", "two"]);
        assert_eq!(response.headers.get("x-new").unwrap(), "added");
        assert_eq!(response.headers.get("x-present").unwrap(), "updated");
        assert!(response.headers.get("x-missing").is_none());
        assert_eq!(response.headers.get("x-empty").unwrap(), "");
        assert!(response.headers.get("x-remove").is_none());
    }

    #[test]
    fn overwrite_action_sets_request_header() {
        let mutation = HeaderMutation {
            set_headers: vec![set_header_opt(
                "x-user",
                "alice",
                HeaderAppendAction::OverwriteIfExistsOrAdd,
            )],
            remove_headers: vec![],
        };
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| n == "x-user" && v == "alice")
            .times(1)
            .returning(|_, _| Ok(()));
        let mut log = PluginLog::new("ExtProc");
        assert_eq!(apply_to_request(&mut session, &mutation, &mut log), 1);
    }

    #[test]
    fn append_action_appends_request_header() {
        let mutation = HeaderMutation {
            set_headers: vec![set_header_opt("x-tag", "a", HeaderAppendAction::AppendIfExistsOrAdd)],
            remove_headers: vec![],
        };
        let mut session = MockPluginSession::new();
        session
            .expect_append_request_header()
            .withf(|n, v| n == "x-tag" && v == "a")
            .times(1)
            .returning(|_, _| Ok(()));
        let mut log = PluginLog::new("ExtProc");
        assert_eq!(apply_to_request(&mut session, &mutation, &mut log), 1);
    }

    #[test]
    fn proxy_managed_headers_are_rejected_on_request() {
        // A processor must not inject framing/hop-by-hop headers onto the
        // upstream request (smuggling guard).
        let mutation = HeaderMutation {
            set_headers: vec![
                set_header_opt("Content-Length", "0", HeaderAppendAction::OverwriteIfExistsOrAdd),
                set_header_opt("Transfer-Encoding", "chunked", HeaderAppendAction::AppendIfExistsOrAdd),
                set_header_opt("Connection", "close", HeaderAppendAction::OverwriteIfExistsOrAdd),
            ],
            remove_headers: vec!["Content-Length".to_string()],
        };
        let mut session = MockPluginSession::new();
        session.expect_set_request_header().never();
        session.expect_append_request_header().never();
        session.expect_remove_request_header().never();
        let mut log = PluginLog::new("ExtProc");
        assert_eq!(apply_to_request(&mut session, &mutation, &mut log), 0);
        assert!(log.contains("err: protected header mutation rejected"));
    }

    #[test]
    fn add_if_absent_skips_when_present() {
        let mutation = HeaderMutation {
            set_headers: vec![set_header_opt("x-id", "1", HeaderAppendAction::AddIfAbsent)],
            remove_headers: vec![],
        };
        let mut session = MockPluginSession::new();
        setup_header_mocks(&mut session, &[("x-id", "existing")]);
        // set_request_header must NOT be called when the header already exists.
        session.expect_set_request_header().never();
        let mut log = PluginLog::new("ExtProc");
        // Conditional no-op: nothing changed, so the applied count is 0.
        assert_eq!(apply_to_request(&mut session, &mutation, &mut log), 0);
    }

    #[test]
    fn pseudo_header_mutation_is_rejected() {
        let mutation = HeaderMutation {
            set_headers: vec![set_header_opt(
                ":method",
                "DELETE",
                HeaderAppendAction::OverwriteIfExistsOrAdd,
            )],
            remove_headers: vec![":path".to_string()],
        };
        let mut session = MockPluginSession::new();
        // No session mutation methods should ever be invoked for pseudo headers.
        session.expect_set_request_header().never();
        session.expect_remove_request_header().never();
        let mut log = PluginLog::new("ExtProc");
        assert_eq!(apply_to_request(&mut session, &mutation, &mut log), 0);
        assert!(log.contains("err: pseudo header mutation rejected"));
    }

    #[test]
    fn remove_headers_removes_response_header() {
        let mutation = HeaderMutation {
            set_headers: vec![],
            remove_headers: vec!["x-internal".to_string()],
        };
        let mut session = MockPluginSession::new();
        session
            .expect_remove_response_header()
            .withf(|n| n == "x-internal")
            .times(1)
            .returning(|_| Ok(()));
        let mut log = PluginLog::new("ExtProc");
        assert_eq!(apply_to_response(&mut session, &mutation, &mut log), 1);
    }

    #[test]
    fn empty_value_skipped_unless_keep_empty_value() {
        // Empty value without keep_empty_value → skipped (no session call).
        let mut skip_opt = set_header_opt("x-empty", "", HeaderAppendAction::OverwriteIfExistsOrAdd);
        skip_opt.keep_empty_value = false;
        let mutation = HeaderMutation {
            set_headers: vec![skip_opt],
            remove_headers: vec![],
        };
        let mut session = MockPluginSession::new();
        session.expect_set_request_header().never();
        let mut log = PluginLog::new("ExtProc");
        assert_eq!(apply_to_request(&mut session, &mutation, &mut log), 0);

        // Empty value WITH keep_empty_value → applied as empty string.
        let mut keep_opt = set_header_opt("x-empty", "", HeaderAppendAction::OverwriteIfExistsOrAdd);
        keep_opt.keep_empty_value = true;
        let mutation = HeaderMutation {
            set_headers: vec![keep_opt],
            remove_headers: vec![],
        };
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| n == "x-empty" && v.is_empty())
            .times(1)
            .returning(|_, _| Ok(()));
        let mut log = PluginLog::new("ExtProc");
        assert_eq!(apply_to_request(&mut session, &mutation, &mut log), 1);
    }

    fn values_for<'a>(map: &'a HeaderMap, key: &str) -> Vec<&'a str> {
        map.headers
            .iter()
            .filter(|h| h.key == key)
            .map(|h| h.value.as_str())
            .collect()
    }

    #[test]
    fn collect_request_headers_includes_pseudo_and_preserves_duplicates() {
        let mut session = MockPluginSession::new();
        session.expect_get_method().return_const("GET".to_string());
        session.expect_get_path().return_const("/api".to_string());
        session
            .expect_header_value()
            .withf(|n| n == "host")
            .returning(|_| Some("example.com".to_string()));
        session.expect_request_headers().returning(|| {
            vec![
                ("x-multi".to_string(), "a".to_string()),
                ("x-multi".to_string(), "b".to_string()),
            ]
        });

        let map = collect_request_headers(&session);
        assert_eq!(values_for(&map, ":method"), vec!["GET"]);
        assert_eq!(values_for(&map, ":path"), vec!["/api"]);
        assert_eq!(values_for(&map, ":authority"), vec!["example.com"]);
        // Duplicate header values are preserved, not collapsed.
        assert_eq!(values_for(&map, "x-multi"), vec!["a", "b"]);
    }

    #[test]
    fn collect_response_headers_includes_status_and_preserves_set_cookie() {
        let mut session = MockPluginSession::new();
        session.expect_get_response_status().return_const(Some(200u16));
        session.expect_response_headers().returning(|| {
            vec![
                ("set-cookie".to_string(), "a=1".to_string()),
                ("set-cookie".to_string(), "b=2".to_string()),
            ]
        });

        let map = collect_response_headers(&session);
        assert_eq!(values_for(&map, ":status"), vec!["200"]);
        // Multi-value Set-Cookie is preserved.
        assert_eq!(values_for(&map, "set-cookie"), vec!["a=1", "b=2"]);
    }

    #[test]
    fn raw_value_preferred_over_value() {
        let mut opt = set_header_opt("x-bin", "ignored", HeaderAppendAction::OverwriteIfExistsOrAdd);
        opt.header.as_mut().unwrap().raw_value = b"raw-wins".to_vec();
        let mutation = HeaderMutation {
            set_headers: vec![opt],
            remove_headers: vec![],
        };
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|n, v| n == "x-bin" && v == "raw-wins")
            .times(1)
            .returning(|_, _| Ok(()));
        let mut log = PluginLog::new("ExtProc");
        assert_eq!(apply_to_request(&mut session, &mutation, &mut log), 1);
    }
}
