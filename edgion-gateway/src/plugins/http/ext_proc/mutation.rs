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
    let mut headers = Vec::new();
    push_pseudo(&mut headers, ":method", session.get_method().to_string());
    push_pseudo(&mut headers, ":path", session.get_uri());
    let request = session.request_header_map();
    if let Some(authority) = request.get("host") {
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
        if reject_protected(&hv.key, log) {
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
        let Some(value) = decoded_value(opt, hv, log) else {
            continue;
        };
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
    use crate::plugins::runtime::session_adapter::record_outcome_tests::test_session;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
    use crate::plugins::runtime::PingoraSessionAdapter;
    use crate::EdgionHttpContext;
    use edgion_resources::path_normalization::PathNormalizationConfig;
    use pingora_http::RequestHeader;
    use prost::Message;

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

    fn raw_opt(key: &str, value: &[u8], action: HeaderAppendAction) -> HeaderValueOption {
        let mut opt = set_header_opt(key, "", action);
        opt.header.as_mut().unwrap().raw_value = value.to_vec();
        opt
    }

    fn wire_round_trip<M: Message + Default>(message: &M) -> M {
        M::decode(message.encode_to_vec().as_slice()).unwrap()
    }

    fn assert_raw_values(map: &http::HeaderMap, key: &str, expected: &[&[u8]]) {
        let values: Vec<_> = map.get_all(key).iter().map(|value| value.as_bytes()).collect();
        assert_eq!(values, expected, "header {key}");
    }

    #[tokio::test]
    async fn opaque_projection_round_trips_request_and_response_field_lines() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::new();
        let mut response = pingora_http::ResponseHeader::build(200, None).unwrap();
        for bytes in [b"\xfffirst".as_slice(), b"second", b""] {
            let value = http::HeaderValue::from_bytes(bytes).unwrap();
            session
                .req_header_mut()
                .append_header("x-opaque", value.clone())
                .unwrap();
            response.append_header("x-opaque", value).unwrap();
        }
        let adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response);
        for map in [collect_request_headers(&adapter), collect_response_headers(&adapter)] {
            let map = wire_round_trip(&map);
            assert!(map.headers.iter().all(|header| header.value.is_empty()));
            let actual: Vec<_> = map
                .headers
                .iter()
                .filter(|header| header.key == "x-opaque")
                .map(|header| header.raw_value.as_slice())
                .collect();
            assert_eq!(actual, [b"\xfffirst".as_slice(), b"second", b""]);
        }
    }

    fn sequential_mutation() -> HeaderMutation {
        let mut empty = raw_opt("x-empty", b"", HeaderAppendAction::OverwriteIfExistsOrAdd);
        empty.keep_empty_value = true;
        wire_round_trip(&HeaderMutation {
            set_headers: vec![
                raw_opt("x-existing", b"must-not-replace", HeaderAppendAction::AddIfAbsent),
                raw_opt("x-existing", b"\x80updated", HeaderAppendAction::OverwriteIfExists),
                raw_opt("x-missing", b"must-not-create", HeaderAppendAction::OverwriteIfExists),
                raw_opt("x-created", b"\x81new", HeaderAppendAction::AddIfAbsent),
                raw_opt("x-created", b"must-not-replace", HeaderAppendAction::AddIfAbsent),
                raw_opt("x-created", b"\x82updated", HeaderAppendAction::OverwriteIfExists),
                raw_opt("x-created", b"\x83append", HeaderAppendAction::AppendIfExistsOrAdd),
                raw_opt(
                    "x-remove",
                    b"\x84set-then-remove",
                    HeaderAppendAction::OverwriteIfExistsOrAdd,
                ),
                raw_opt("content-length", b"999", HeaderAppendAction::OverwriteIfExistsOrAdd),
                raw_opt(":method", b"DELETE", HeaderAppendAction::OverwriteIfExistsOrAdd),
                empty,
            ],
            remove_headers: vec![
                "x-remove".to_string(),
                "content-length".to_string(),
                ":path".to_string(),
            ],
        })
    }

    fn initial_mutation_headers() -> [(&'static str, http::HeaderValue); 3] {
        [
            ("x-existing", http::HeaderValue::from_bytes(b"\xfforiginal").unwrap()),
            ("x-existing", http::HeaderValue::from_static("later-text")),
            ("content-length", http::HeaderValue::from_static("0")),
        ]
    }

    fn assert_sequential_result(map: &http::HeaderMap) {
        assert_raw_values(map, "x-existing", &[b"\x80updated"]);
        assert_raw_values(map, "x-created", &[b"\x82updated", b"\x83append"]);
        assert_raw_values(map, "x-empty", &[b""]);
        assert!(!map.contains_key("x-missing"));
        assert!(!map.contains_key("x-remove"));
        assert_eq!(map["content-length"], "0");
    }

    #[tokio::test]
    async fn opaque_mutations_observe_live_sequential_presence_in_both_directions() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::new();
        let mut response = pingora_http::ResponseHeader::build(200, None).unwrap();
        for (name, value) in initial_mutation_headers() {
            session.req_header_mut().append_header(name, value.clone()).unwrap();
            response.append_header(name, value).unwrap();
        }
        let mutation = sequential_mutation();
        {
            let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response);
            let mut log = PluginLog::new("ExtProc");
            assert_eq!(apply_to_request(&mut adapter, &mutation, &mut log), 7);
            assert_eq!(apply_to_response(&mut adapter, &mutation, &mut log), 7);
            assert!(log.contains("protected header mutation rejected"));
        }
        assert_sequential_result(&session.req_header().headers);
        assert_sequential_result(&response.headers);
        assert_eq!(session.req_header().method.as_str(), "GET");
    }

    #[tokio::test]
    async fn opaque_local_reply_round_trip_defers_conditions_to_fresh_headers() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::new();
        let mut upstream = pingora_http::ResponseHeader::build(200, None).unwrap();
        upstream.insert_header("x-created", "upstream-only").unwrap();
        let mut reply = pingora_http::ResponseHeader::build(403, None).unwrap();
        for (name, value) in initial_mutation_headers() {
            reply.append_header(name, value).unwrap();
        }
        {
            let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut upstream);
            let mut log = PluginLog::new("ExtProc");
            assert_eq!(
                queue_for_local_reply(&mut adapter, &sequential_mutation(), &mut log),
                10
            );
        }
        assert_eq!(upstream.headers["x-created"], "upstream-only");
        ctx.drain_pending_response_header_ops(&mut reply);
        assert_sequential_result(&reply.headers);
        assert!(!ctx.has_pending_response_header_ops());
    }

    #[test]
    fn invalid_processor_bytes_are_skipped_without_exposing_contents() {
        for bytes in [b"private\rvalue".as_slice(), b"private\nvalue", b"private\0value"] {
            let mutation = wire_round_trip(&HeaderMutation {
                set_headers: vec![raw_opt("x-private", bytes, HeaderAppendAction::OverwriteIfExistsOrAdd)],
                remove_headers: vec![],
            });
            let mut session = MockPluginSession::new();
            let mut log = PluginLog::new("ExtProc");
            assert_eq!(apply_to_request(&mut session, &mutation, &mut log), 0);
            assert_eq!(apply_to_response(&mut session, &mutation, &mut log), 0);
            assert!(local_reply_ops(&mutation, &mut log).is_empty());
            assert!(log.contains("invalid header value"));
            assert!(!log.contains("private"));
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
            .expect_set_request_header_value()
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
            .expect_append_request_header_value()
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
        session.expect_set_request_header_value().never();
        session.expect_append_request_header_value().never();
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
        session.expect_set_request_header_value().never();
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
        session.expect_set_request_header_value().never();
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
        session.expect_set_request_header_value().never();
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
            .expect_set_request_header_value()
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
            .map(|h| std::str::from_utf8(&h.raw_value).unwrap())
            .collect()
    }

    #[test]
    fn collect_request_headers_includes_pseudo_and_preserves_duplicates() {
        let mut session = MockPluginSession::new();
        session.expect_get_method().return_const("GET".to_string());
        session.expect_get_uri().times(1).return_const("/api".to_string());
        setup_header_mocks(
            &mut session,
            &[("host", "example.com"), ("x-multi", "a"), ("x-multi", "b")],
        );

        let map = collect_request_headers(&session);
        assert_eq!(values_for(&map, ":method"), vec!["GET"]);
        assert_eq!(values_for(&map, ":path"), vec!["/api"]);
        assert_eq!(values_for(&map, ":authority"), vec!["example.com"]);
        // Duplicate header values are preserved, not collapsed.
        assert_eq!(values_for(&map, "x-multi"), vec!["a", "b"]);
    }

    #[test]
    fn collect_request_headers_preserves_current_uri_and_raw_query() {
        for (current_uri, expected) in [
            ("/api?mode=deny", "/api?mode=deny"),
            (
                "/api?tag=one&tag=two&value=foo%26bar&text=hello+world",
                "/api?tag=one&tag=two&value=foo%26bar&text=hello+world",
            ),
            // The canonical URI accessor omits an empty query delimiter.
            ("/api?", "/api"),
            ("/api", "/api"),
            ("/%61pi?mode=deny", "/api?mode=deny"),
        ] {
            let mut ctx = EdgionHttpContext::new();
            ctx.request_info.entry_raw_path = "/entry".to_string();
            ctx.request_info.entry_raw_query = Some("mode=allow".to_string());
            let request = RequestHeader::build("GET", current_uri.as_bytes(), None).unwrap();
            ctx.refresh_normalized_path(&request, &PathNormalizationConfig::default());
            assert_eq!(ctx.path(), "/api");
            assert_eq!(ctx.uri(&request), expected);
            assert_eq!(ctx.request_uri(), "/entry?mode=allow");
            assert_ne!(ctx.uri(&request), ctx.request_uri());

            let mut session = MockPluginSession::new();
            session.expect_get_method().return_const("GET".to_string());
            session.expect_get_uri().times(1).returning(move || ctx.uri(&request));
            session.expect_get_path().never();
            session.expect_get_request_path().never();
            session.expect_get_request_uri().never();
            session.expect_get_query().never();
            session.expect_request_header_map().return_const(http::HeaderMap::new());

            let map = collect_request_headers(&session);
            assert_eq!(values_for(&map, ":path"), vec![expected], "current URI: {current_uri}");
        }
    }

    #[test]
    fn collect_response_headers_includes_status_and_preserves_set_cookie() {
        let mut session = MockPluginSession::new();
        session.expect_get_response_status().return_const(Some(200u16));
        session.expect_response_headers_bytes().returning(|| {
            vec![
                (b"set-cookie".to_vec(), b"a=1".to_vec()),
                (b"set-cookie".to_vec(), b"b=2".to_vec()),
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
            .expect_set_request_header_value()
            .withf(|n, v| n == "x-bin" && v == "raw-wins")
            .times(1)
            .returning(|_, _| Ok(()));
        let mut log = PluginLog::new("ExtProc");
        assert_eq!(apply_to_request(&mut session, &mutation, &mut log), 1);
    }
}
