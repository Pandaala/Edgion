use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::PluginLog;
use edgion_resources::plugin_configs::WasmConfig;
use edgion_resources::plugin_configs::WasmRawConfig;
use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;

/// Guest whose request-headers hook sets `x-wat-added: yes`, and whose
/// response-headers hook sends a 403 local response with body "denied"
/// (the response-headers hook is unused by these request-phase tests).
///
/// Duplicated from `host_abi.rs`'s test-only `GUEST` constant: that
/// constant lives inside `host_abi`'s own `#[cfg(test)] mod tests` and is
/// not visible here.
const GUEST_SET_HEADER: &str = r#"
    (module
      (import "env" "proxy_replace_header_map_value"
        (func $set_hdr (param i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_send_local_response"
        (func $slr (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (global $heap (mut i32) (i32.const 4096))
      ;; offsets: 0: "x-wat-added" (11), 16: "yes" (3), 32: "denied" (6)
      (data (i32.const 0) "x-wat-added")
      (data (i32.const 16) "yes")
      (data (i32.const 32) "denied")
      (func (export "proxy_on_memory_allocate") (param i32) (result i32)
        (local $p i32)
        global.get $heap
        local.set $p
        global.get $heap
        local.get 0
        i32.add
        global.set $heap
        local.get $p)
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
        (drop (call $set_hdr (i32.const 0) (i32.const 0) (i32.const 11) (i32.const 16) (i32.const 3)))
        (i32.const 0))
      (func (export "proxy_on_response_headers") (param i32 i32 i32) (result i32)
        (drop (call $slr (i32.const 403) (i32.const 0) (i32.const 0)
                         (i32.const 32) (i32.const 6) (i32.const 0) (i32.const 0) (i32.const 0)))
        (i32.const 0))
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

/// Guest whose request-headers hook hits `unreachable` (simulated panic).
///
/// Duplicated from `vm_manager.rs`'s test-only `TRAP_GUEST` constant for
/// the same visibility reason as `GUEST_SET_HEADER` above.
const GUEST_TRAP: &str = r#"
    (module
      (memory (export "memory") 1)
      (func (export "proxy_on_memory_allocate") (param i32) (result i32) (i32.const 1024))
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) unreachable)
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

/// The Rust SDK declares some standard ABI imports that Edgion does not
/// implement. They are load-compatible for SDK portability, but invoking
/// one must trap and enter the ordinary guest-trap failure policy.
const GUEST_KNOWN_UNSUPPORTED_TRAP: &str = r#"
    (module
      (import "env" "proxy_close_stream" (func $close (param i32) (result i32)))
      (memory (export "memory") 1)
      (func (export "proxy_on_memory_allocate") (param i32) (result i32) (i32.const 1024))
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
        (call $close (i32.const 0)))
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
      (func (export "proxy_abi_version_0_2_0"))
    )
    "#;

/// Callout demo guest: on request headers it reads the target origin
/// from the `x-callout-url` request header, dispatches a GET /check
/// callout to it, and returns Pause. On the callout response it resumes
/// the request when the callout completed (num_headers > 0) or sends a
/// 403 local response when the callout failed. A dispatch rejection
/// (denied origin) is answered immediately with a 503 local response.
///
/// The embedded 69-byte data blob at offset 32 is the proxy-wasm
/// serialized header map for [(":method","GET"), (":path","/check"),
/// (":authority","test")].
const GUEST_CALLOUT: &str = r#"
    (module
      (import "env" "proxy_get_header_map_value"
        (func $get_hdr (param i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_dispatch_http_call"
        (func $dispatch (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_continue_request" (func $cont (result i32)))
      (import "env" "proxy_send_local_response"
        (func $slr (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (global $heap (mut i32) (i32.const 8192))
      (data (i32.const 0) "x-callout-url")
      (data (i32.const 32) "\03\00\00\00\07\00\00\00\03\00\00\00\05\00\00\00\06\00\00\00\0a\00\00\00\04\00\00\00:method\00GET\00:path\00/check\00:authority\00test\00")
      (data (i32.const 128) "denied")
      (func (export "proxy_on_memory_allocate") (param i32) (result i32)
        (local $p i32)
        global.get $heap
        local.set $p
        global.get $heap
        local.get 0
        i32.add
        global.set $heap
        local.get $p)
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
        (if (i32.ne (call $get_hdr (i32.const 0) (i32.const 0) (i32.const 13)
                              (i32.const 256) (i32.const 260)) (i32.const 0))
          (then
            (drop (call $slr (i32.const 500) (i32.const 0) (i32.const 0)
                             (i32.const 128) (i32.const 6) (i32.const 0) (i32.const 0) (i32.const 0)))
            (return (i32.const 0))))
        (if (i32.ne (call $dispatch (i32.load (i32.const 256)) (i32.load (i32.const 260))
                              (i32.const 32) (i32.const 69)
                              (i32.const 0) (i32.const 0) (i32.const 0) (i32.const 0)
                              (i32.const 1000) (i32.const 264)) (i32.const 0))
          (then
            (drop (call $slr (i32.const 503) (i32.const 0) (i32.const 0)
                             (i32.const 128) (i32.const 6) (i32.const 0) (i32.const 0) (i32.const 0)))
            (return (i32.const 0))))
        (i32.const 1))
      (func (export "proxy_on_http_call_response") (param i32 i32 i32 i32 i32)
        (if (i32.gt_s (local.get 2) (i32.const 0))
          (then (drop (call $cont)))
          (else (drop (call $slr (i32.const 403) (i32.const 0) (i32.const 0)
                                 (i32.const 128) (i32.const 6) (i32.const 0) (i32.const 0) (i32.const 0))))))
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

/// Same dispatch flow as `GUEST_CALLOUT`, but the callout response
/// handler neither resumes nor answers — exercising the host's
/// no-resume fail-mode fallback.
const GUEST_CALLOUT_NO_RESUME: &str = r#"
    (module
      (import "env" "proxy_get_header_map_value"
        (func $get_hdr (param i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_dispatch_http_call"
        (func $dispatch (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (global $heap (mut i32) (i32.const 8192))
      (data (i32.const 0) "x-callout-url")
      (data (i32.const 32) "\03\00\00\00\07\00\00\00\03\00\00\00\05\00\00\00\06\00\00\00\0a\00\00\00\04\00\00\00:method\00GET\00:path\00/check\00:authority\00test\00")
      (func (export "proxy_on_memory_allocate") (param i32) (result i32)
        (local $p i32)
        global.get $heap
        local.set $p
        global.get $heap
        local.get 0
        i32.add
        global.set $heap
        local.get $p)
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
        (drop (call $get_hdr (i32.const 0) (i32.const 0) (i32.const 13)
                             (i32.const 256) (i32.const 260)))
        (drop (call $dispatch (i32.load (i32.const 256)) (i32.load (i32.const 260))
                              (i32.const 32) (i32.const 69)
                              (i32.const 0) (i32.const 0) (i32.const 0) (i32.const 0)
                              (i32.const 1000) (i32.const 264)))
        (i32.const 1))
      (func (export "proxy_on_http_call_response") (param i32 i32 i32 i32 i32))
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

fn write_temp_wasm(wat: &str) -> tempfile::NamedTempFile {
    let bytes = wat::parse_str(wat).expect("wat parses");
    let f = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(f.path(), &bytes).unwrap();
    f
}

fn config_for(path: &std::path::Path, fail_open: bool) -> WasmConfig {
    let mut c = WasmConfig::default();
    c.source.file = Some(path.to_string_lossy().into_owned());
    c.fail_open = fail_open;
    c
}

fn base_mock() -> MockPluginSession {
    let mut s = MockPluginSession::new();
    s.expect_request_headers()
        .returning(|| vec![("host".to_string(), "t.example.com".to_string())]);
    s.expect_get_path().return_const("/x".to_string());
    s.expect_get_method().return_const("GET".to_string());
    s.expect_header_value().returning(|_| None);
    s.expect_remote_ip().return_const("1.2.3.4".to_string());
    // Default request-body-phase stubs: a WAT guest without a
    // `proxy_on_request_body` export never reaches these, but the real
    // proxy-wasm SDK guest does. Ungated (no `body:`) → `read_request_body`
    // returns `Unimplemented`, which now routes through `fail_mode` (Task
    // 7) rather than being skipped -- most callers of `base_mock` never
    // hit this arm at all since their WAT guests lack the export. Tests
    // that need an actual buffered body use `body_mock` instead.
    s.expect_ctx().return_const(crate::EdgionHttpContext::new());
    s.expect_read_request_body()
        .returning(|_| Err(crate::plugins::runtime::traits::session::BodyReadError::Unimplemented));
    s
}

#[tokio::test]
async fn request_phase_applies_header_set() {
    let f = write_temp_wasm(GUEST_SET_HEADER);
    let plugin = actual_request(&config_for(f.path(), false)).expect("plugin");
    let mut session = base_mock();
    session
        .expect_set_request_header()
        .withf(|k, v| k == "x-wat-added" && v == "yes")
        .times(1)
        .returning(|_, _| Ok(()));
    let mut log = PluginLog::new("Wasm");
    let res = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(res, PluginRunningResult::GoodNext);
}

#[tokio::test]
async fn separate_plugin_instances_isolate_state_and_drop_independently() {
    let f = write_temp_wasm(GUEST_SET_HEADER);
    let config = config_for(f.path(), false);
    let first = actual_plugin(&config);
    let second = actual_plugin(&config);
    let first_entry = first.runtime.get_or_load().await.expect("first runtime loads");
    let second_entry = second.runtime.get_or_load().await.expect("second runtime loads");
    assert!(!Arc::ptr_eq(&first_entry, &second_entry));
    first_entry
        .shared_data_for_test()
        .set("owner", Some(b"first".to_vec()), 0)
        .expect("write first runtime state");
    assert!(second_entry.shared_data_for_test().get("owner").is_none());
    drop(first_entry);
    drop(first);

    let mut session = base_mock();
    session
        .expect_set_request_header()
        .withf(|name, value| name == "x-wat-added" && value == "yes")
        .times(1)
        .returning(|_, _| Ok(()));
    let mut log = PluginLog::new("Wasm");
    assert_eq!(
        second.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert!(second_entry.shared_data_for_test().get("owner").is_none());
}

#[tokio::test]
async fn trap_fail_closed_returns_500() {
    let f = write_temp_wasm(GUEST_TRAP);
    let plugin = actual_request(&config_for(f.path(), false)).unwrap();
    let mut session = base_mock();
    // Task 9: the VM-trap fail-close arm records `FailClose` with the
    // kebab-cased reason ("trap") as cause.
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "trap")
        .times(1)
        .return_const(());
    let mut log = PluginLog::new("Wasm");
    let res = plugin.run_request(&mut session, &mut log).await;
    assert!(matches!(res, PluginRunningResult::ErrResponse { status: 500, .. }));
}

#[tokio::test]
async fn trap_fail_open_continues() {
    let f = write_temp_wasm(GUEST_TRAP);
    let plugin = actual_request(&config_for(f.path(), true)).unwrap();
    let mut session = base_mock();
    // Task 9: the VM-trap fail-open arm records `FailOpen` with the same cause.
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "trap")
        .times(1)
        .return_const(());
    let mut log = PluginLog::new("Wasm");
    let res = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(res, PluginRunningResult::GoodNext);
}

#[tokio::test]
async fn known_unsupported_sdk_import_traps_through_failure_policy() {
    let f = write_temp_wasm(GUEST_KNOWN_UNSUPPORTED_TRAP);
    let plugin = actual_request(&config_for(f.path(), false)).unwrap();
    let mut session = base_mock();
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "trap")
        .times(1)
        .return_const(());
    let mut log = PluginLog::new("Wasm");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
}

#[tokio::test]
async fn gateway_survives_trap_then_serves_next_request() {
    let f = write_temp_wasm(GUEST_TRAP);
    let plugin = actual_request(&config_for(f.path(), true)).unwrap();
    for _ in 0..3 {
        let mut session = base_mock();
        session.expect_record_outcome().return_const(());
        let mut log = PluginLog::new("Wasm");
        let _ = plugin.run_request(&mut session, &mut log).await;
    }
    // reaching here without panic = recovery works
}

#[tokio::test]
async fn load_failure_fail_closed() {
    let mut c = WasmConfig::default();
    c.source.file = Some("/nonexistent/missing.wasm".to_string());
    let plugin = actual_request(&c).unwrap();
    let mut session = base_mock();
    // Task 9: the module-load-failure fail-close arm records `FailClose`
    // with cause "load".
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "load")
        .times(1)
        .return_const(());
    let mut log = PluginLog::new("Wasm");
    let res = plugin.run_request(&mut session, &mut log).await;
    assert!(matches!(res, PluginRunningResult::ErrResponse { status: 500, .. }));
}

/// The guest writes raw bytes over the Proxy-Wasm ABI, so a binary local
/// response must reach the client verbatim. The previous `String` body
/// round-tripped through `from_utf8_lossy`, which both copied the buffer
/// and replaced every invalid sequence with U+FFFD.
#[test]
fn local_response_forwards_non_utf8_body_verbatim() {
    const RAW: &[u8] = &[0x89, 0x50, 0x4e, 0x47, 0xff, 0x00];

    let mut session = MockPluginSession::new();
    session.expect_set_response_header().returning(|_, _| Ok(()));
    session.expect_append_response_header().returning(|_, _| Ok(()));

    let local = LocalResponse {
        status: 200,
        body: RAW.to_vec(),
        headers: vec![("content-type".to_string(), "image/png".to_string())],
    };
    let mut log = PluginLog::new("Wasm");
    match finish_local_response(&mut session, &mut log, local) {
        PluginRunningResult::ErrResponse { status, body } => {
            assert_eq!(status, 200);
            assert_eq!(body.expect("binary body").as_ref(), RAW);
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

/// An empty guest body stays *absent*, not a present zero-length body, so
/// the runtime writes a bodyless response header.
#[test]
fn local_response_empty_guest_body_is_absent() {
    let mut session = MockPluginSession::new();
    session.expect_set_response_header().returning(|_, _| Ok(()));
    session.expect_append_response_header().returning(|_, _| Ok(()));

    let local = LocalResponse {
        status: 204,
        body: Vec::new(),
        headers: Vec::new(),
    };
    let mut log = PluginLog::new("Wasm");
    match finish_local_response(&mut session, &mut log, local) {
        PluginRunningResult::ErrResponse { status, body } => {
            assert_eq!(status, 204);
            assert!(body.is_none());
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

/// A WASM guest local response that repeats a header name (two Set-Cookie
/// pairs) must not collapse to the last value. Reproduces the request-stage
/// session adapter (no real ResponseHeader yet → ops are queued) and drains
/// the queue onto a real ResponseHeader, asserting both cookies survive.
#[test]
fn local_response_preserves_duplicate_set_cookie() {
    use crate::ctx::PendingHeaderOp;
    use parking_lot::Mutex;
    use pingora_http::ResponseHeader;
    use std::sync::Arc;

    let queue: Arc<Mutex<Vec<PendingHeaderOp>>> = Arc::new(Mutex::new(Vec::new()));
    let mut session = MockPluginSession::new();
    let q_set = queue.clone();
    session.expect_set_response_header().returning(move |name, value| {
        q_set.lock().push(PendingHeaderOp::Set {
            name: name.to_string(),
            value: http::HeaderValue::from_str(value).unwrap(),
        });
        Ok(())
    });
    let q_app = queue.clone();
    session.expect_append_response_header().returning(move |name, value| {
        q_app.lock().push(PendingHeaderOp::Append {
            name: name.to_string(),
            value: http::HeaderValue::from_str(value).unwrap(),
        });
        Ok(())
    });

    // Mixed case on the repeated name exercises the case-insensitive dedup.
    let local = LocalResponse {
        status: 302,
        body: Vec::new(),
        headers: vec![
            ("Set-Cookie".to_string(), "session=abc".to_string()),
            ("set-cookie".to_string(), "refresh=xyz".to_string()),
            ("location".to_string(), "/login".to_string()),
        ],
    };
    let mut log = PluginLog::new("Wasm");
    let res = finish_local_response(&mut session, &mut log, local);
    assert!(matches!(res, PluginRunningResult::ErrResponse { status: 302, .. }));

    // Drain the queued ops onto a real ResponseHeader (the path a
    // request-stage local reply actually takes) and assert both survived.
    let mut resp = ResponseHeader::build(302, None).unwrap();
    for op in queue.lock().drain(..) {
        op.apply(&mut resp);
    }
    let cookies: Vec<String> = resp
        .headers
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert_eq!(cookies, vec!["session=abc".to_string(), "refresh=xyz".to_string()]);
    // A single-valued header is unaffected by the set-then-append change.
    assert_eq!(resp.headers.get("location").unwrap().to_str().unwrap(), "/login");
}

/// Mock session for the callout guests: carries the callout target in
/// the `x-callout-url` request header.
fn callout_mock(target: &str) -> MockPluginSession {
    let mut s = MockPluginSession::new();
    let target = target.to_string();
    s.expect_request_headers().returning(move || {
        vec![
            ("host".to_string(), "t.example.com".to_string()),
            ("x-callout-url".to_string(), target.clone()),
        ]
    });
    s.expect_get_path().return_const("/x".to_string());
    s.expect_get_method().return_const("GET".to_string());
    s.expect_header_value().returning(|_| None);
    s.expect_remote_ip().return_const("1.2.3.4".to_string());
    s
}

fn callout_config(path: &std::path::Path, allowlist: Vec<String>, fail_open: bool) -> WasmConfig {
    let mut c = config_for(path, fail_open);
    c.callout_allowlist = allowlist;
    c
}

/// Callout happy path: guest pauses on a dispatched callout, the target
/// answers 200, guest resumes → request proceeds. Verifies the callout
/// actually hit the target with the guest-supplied path and authority.
#[tokio::test]
async fn callout_success_resumes_request() {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/check"))
        .and(header("host", "test"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ok".to_vec()))
        .mount(&server)
        .await;

    let f = write_temp_wasm(GUEST_CALLOUT);
    let config = callout_config(f.path(), vec![server.uri()], false);
    let plugin = actual_request(&config).unwrap();
    let mut session = callout_mock(&server.uri());
    let mut log = PluginLog::new("Wasm");
    let res = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(res, PluginRunningResult::GoodNext);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

/// Callout target unreachable → failure callback (0 headers) → guest
/// sends a 403 local response.
#[tokio::test]
async fn callout_failure_guest_denies() {
    let f = write_temp_wasm(GUEST_CALLOUT);
    // Port 1 is unreachable; the origin is allowlisted so dispatch
    // succeeds and the failure surfaces via the callback.
    let target = "http://127.0.0.1:1";
    let config = callout_config(f.path(), vec![target.to_string()], false);
    let plugin = actual_request(&config).unwrap();
    let mut session = callout_mock(target);
    let mut log = PluginLog::new("Wasm");
    let res = plugin.run_request(&mut session, &mut log).await;
    assert!(
        matches!(res, PluginRunningResult::ErrResponse { status: 403, .. }),
        "got {res:?}"
    );
}

/// Origin not in the allowlist → dispatch returns a host error the
/// guest observes (here: it answers 503 itself).
#[tokio::test]
async fn callout_denied_by_allowlist() {
    let f = write_temp_wasm(GUEST_CALLOUT);
    let config = callout_config(f.path(), vec!["https://allowed.example.com".to_string()], false);
    let plugin = actual_request(&config).unwrap();
    let mut session = callout_mock("http://127.0.0.1:9999");
    let mut log = PluginLog::new("Wasm");
    let res = plugin.run_request(&mut session, &mut log).await;
    assert!(
        matches!(res, PluginRunningResult::ErrResponse { status: 503, .. }),
        "got {res:?}"
    );
}

/// Guest pauses on a callout but never resumes → host fail-mode
/// fallback: failClose returns 500, failOpen continues.
#[tokio::test]
async fn callout_no_resume_falls_back_to_fail_mode() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/check"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let f = write_temp_wasm(GUEST_CALLOUT_NO_RESUME);
    for (fail_open, expect_500) in [(false, true), (true, false)] {
        let config = callout_config(f.path(), vec![server.uri()], fail_open);
        let plugin = actual_request(&config).unwrap();
        let mut session = callout_mock(&server.uri());
        // Task 9: the no-resume fail-mode fallback records the matching
        // outcome with cause "no-resume".
        let expected_outcome = if fail_open {
            DyeOutcome::FailOpen
        } else {
            DyeOutcome::FailClose
        };
        session
            .expect_record_outcome()
            .withf(move |outcome, value| *outcome == expected_outcome && value == "no-resume")
            .times(1)
            .return_const(());
        let mut log = PluginLog::new("Wasm");
        let res = plugin.run_request(&mut session, &mut log).await;
        if expect_500 {
            assert!(
                matches!(res, PluginRunningResult::ErrResponse { status: 500, .. }),
                "failClose must 500, got {res:?}"
            );
        } else {
            assert_eq!(res, PluginRunningResult::GoodNext, "failOpen must continue");
        }
    }
}

/// Body-observation guest: `proxy_on_response_body` reads the
/// current chunk window via `get_buffer_bytes(HttpResponseBody)` and
/// returns the number of bytes it saw (a test-only contract that lets
/// the host assert the window was visible).
const GUEST_BODY_OBSERVER: &str = r#"
    (module
      (import "env" "proxy_get_buffer_bytes"
        (func $get_buf (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (global $heap (mut i32) (i32.const 8192))
      (func (export "proxy_on_memory_allocate") (param i32) (result i32)
        (local $p i32)
        global.get $heap
        local.set $p
        global.get $heap
        local.get 0
        i32.add
        global.set $heap
        local.get $p)
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
      (func (export "proxy_on_response_body") (param i32 i32 i32) (result i32)
        (drop (call $get_buf (i32.const 1) (i32.const 0) (i32.const 65536)
                             (i32.const 256) (i32.const 260)))
        (i32.load (i32.const 260)))
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

/// Body-observation: the guest sees exactly the current chunk window.
#[tokio::test]
async fn body_observation_guest_sees_chunk_window() {
    use crate::plugins::http::wasm::vm_manager::WasmVmManager;
    let f = write_temp_wasm(GUEST_BODY_OBSERVER);
    let c = config_for(f.path(), false);
    let mgr = WasmVmManager::new_for_test();
    let runtime = super::super::vm_manager::tests::actual_runtime(&mgr, &c, CalloutGrant::none());
    let entry = runtime.get_or_load().await.expect("loads");
    let mut lease = entry.acquire(100).expect("lease");
    lease.state_mut().begin_phase(
        WasmPhase::ResponseBody,
        Vec::new(),
        Vec::new(),
        std::collections::HashMap::new(),
    );
    lease.state_mut().body_chunk = Bytes::from_static(b"hello-chunk");
    let seen = lease.call_on_response_body(11, false).expect("hook ok");
    assert_eq!(seen, 11, "guest must observe exactly the staged chunk bytes");

    // Next chunk replaces the window — no accumulation.
    lease.state_mut().begin_phase(
        WasmPhase::ResponseBody,
        Vec::new(),
        Vec::new(),
        std::collections::HashMap::new(),
    );
    lease.state_mut().body_chunk = Bytes::from_static(b"x");
    let seen = lease.call_on_response_body(1, true).expect("hook ok");
    assert_eq!(seen, 1, "window must be the current chunk only");
}

/// Guest whose body hook returns Action::Pause (1) — i.e. it is written to
/// the standard proxy-wasm cumulative-buffer idiom this host does not
/// implement.
const GUEST_BODY_PAUSE: &str = r#"
    (module
      (memory (export "memory") 1)
      (func (export "proxy_on_memory_allocate") (param i32) (result i32) (i32.const 1024))
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
      (func (export "proxy_on_response_body") (param i32 i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

/// A guest that Pauses on the body hook expects cumulative buffering, which
/// this host cannot provide. The stream must keep flowing (never throttle,
/// never break); the incompatibility is surfaced only via the bounded
/// `edgion_wasm_body_pause_unsupported_total` metric (no tracing on the
/// per-chunk hot path — Iron rule 3).
#[tokio::test]
async fn body_pause_is_metered_and_never_breaks_the_stream() {
    let f = write_temp_wasm(GUEST_BODY_PAUSE);
    let c = config_for(f.path(), false);

    // The Pausing guest exports a body hook, so the body-filter stage will
    // lease and run it (rather than skipping header-only guests).
    let plugin = actual_plugin(&c);
    let entry = plugin.runtime.get_or_load().await.expect("loads");
    assert!(entry.has_body_hook(), "guest exports proxy_on_response_body");

    // Plugin path: a Pausing guest must not throttle or break the stream.
    let mut session = MockPluginSession::new();
    let chunk = Some(bytes::Bytes::from_static(b"abc"));
    for eos in [false, true] {
        assert!(
            plugin
                .run_upstream_response_body_filter(&chunk, eos, &mut session)
                .is_none(),
            "a Pausing body guest must never throttle or interrupt the stream"
        );
    }
}

#[tokio::test]
async fn body_filter_releases_chunk_before_returning_lease_to_pool() {
    let f = write_temp_wasm(GUEST_BODY_OBSERVER);
    let c = config_for(f.path(), false);
    let plugin = actual_plugin(&c);
    let entry = plugin.runtime.get_or_load().await.expect("module loads");
    let mut session = MockPluginSession::new();
    let chunk = Bytes::from(vec![1, 2, 3, 4]);
    let body = Some(chunk.clone());

    assert!(plugin
        .run_upstream_response_body_filter(&body, false, &mut session)
        .is_none());
    drop(body);

    let lease = entry.acquire(100).expect("reacquire pooled VM");
    assert!(lease.state().body_chunk.is_empty());
    drop(lease);
    assert!(
        chunk.try_into_mut().is_ok(),
        "the returned lease must not retain a reference to the response chunk"
    );
}

/// Guest whose body hook traps — the stream must keep flowing.
const GUEST_BODY_TRAP: &str = r#"
    (module
      (memory (export "memory") 1)
      (func (export "proxy_on_memory_allocate") (param i32) (result i32) (i32.const 1024))
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
      (func (export "proxy_on_response_body") (param i32 i32 i32) (result i32) unreachable)
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

/// Plugin-level body filter: never throttles, never panics — including
/// with guests that have no body hook and one whose body hook traps.
#[tokio::test]
async fn body_filter_is_nonintrusive() {
    for wat in [GUEST_BODY_OBSERVER, GUEST_SET_HEADER, GUEST_TRAP, GUEST_BODY_TRAP] {
        let f = write_temp_wasm(wat);
        let c = config_for(f.path(), false);
        // Warm this plugin's runtime so the sync lookup hits.
        let plugin = actual_plugin(&c);
        plugin.runtime.get_or_load().await.expect("loads");
        let mut session = MockPluginSession::new();
        let chunk = Some(bytes::Bytes::from_static(b"data"));
        let r = plugin.run_upstream_response_body_filter(&chunk, false, &mut session);
        assert!(r.is_none(), "body observation must never throttle");
        let r = plugin.run_upstream_response_body_filter(&None, true, &mut session);
        assert!(r.is_none());
    }
}

/// First real proxy-wasm-SDK guest through the host: the reference
/// `header-demo` module built by `edgion-tests/wasm/build.sh`. Verifies
/// the request-phase contract (`x-wasm-added: yes`) documented in
/// `edgion-tests/wasm/README.md`.
#[tokio::test]
async fn reference_guest_module_loads_and_runs() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../edgion-tests/wasm/artifacts/header_demo.wasm"
    );
    if !std::path::Path::new(path).exists() {
        eprintln!("skipping: reference artifact not built");
        return;
    }
    let mut c = WasmConfig::default();
    c.source.file = Some(path.to_string());
    // This test is about the header phase, not body-read policy; the SDK
    // guest also exports `proxy_on_request_body`, and `base_mock`'s
    // `Unimplemented` stub now runs through `fail_mode` (Task 7), so
    // `fail_open` must be set to keep that phase a no-op here.
    c.fail_open = true;
    let plugin = actual_request(&c).unwrap();
    let mut session = base_mock();
    session
        .expect_set_request_header()
        .withf(|k, v| k == "x-wasm-added" && v == "yes")
        .times(1)
        .returning(|_, _| Ok(()));
    session.expect_record_outcome().return_const(());
    // The SDK guest's request-body hook runs `read_request_body` ->
    // `Unimplemented` -> `fail_mode` (Task 7), which now also records the
    // matching `EdgionErrorCode` (R17) before `fail_open` softens the result.
    session.expect_record_error().return_const(());
    let mut log = PluginLog::new("Wasm");
    let res = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(res, PluginRunningResult::GoodNext);
}

/// Response-phase coverage for the same reference guest: verifies the
/// `x-wasm-resp: yes` contract. The sync `run_upstream_response_filter`
/// hook uses a sync lookup, so warm the same plugin's runtime first.
#[tokio::test]
async fn reference_guest_response_phase_sets_header() {
    let src_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../edgion-tests/wasm/artifacts/header_demo.wasm"
    );
    if !std::path::Path::new(src_path).exists() {
        eprintln!("skipping: reference artifact not built");
        return;
    }
    let bytes = std::fs::read(src_path).expect("read reference artifact");
    let f = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(f.path(), &bytes).unwrap();

    let mut c = WasmConfig::default();
    c.source.file = Some(f.path().to_string_lossy().into_owned());

    let plugin = actual_plugin(&c);
    plugin.runtime.get_or_load().await.expect("module loads");
    let mut session = base_mock();
    session.expect_response_headers().returning(Vec::new);
    session.expect_get_response_status().return_const(Some(200u16));
    session
        .expect_set_response_header()
        .withf(|k, v| k == "x-wasm-resp" && v == "yes")
        .times(1)
        .returning(|_, _| Ok(()));
    let mut log = PluginLog::new("Wasm");
    let res = plugin.run_upstream_response_filter(&mut session, &mut log);
    assert_eq!(res, PluginRunningResult::GoodNext);
}

// ── Reference SDK guest: new-capability coverage ─────────────────────────
//
// These drive the REAL rebuilt `header_demo.wasm` (proxy-wasm Rust SDK)
// through the host for callout / tick+shared-data / body observation — the
// same guest the integration suite uses, so they catch SDK-vs-host ABI
// drift the WAT fixtures cannot. Every plugin owns independent mutable
// runtime state even when tests use the same artifact.

const REF_ARTIFACT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../edgion-tests/wasm/artifacts/header_demo.wasm"
);

/// Copy the committed reference artifact to a fresh temp file.
/// Returns None when the artifact has not been built.
fn ref_artifact_temp() -> Option<tempfile::NamedTempFile> {
    if !std::path::Path::new(REF_ARTIFACT).exists() {
        eprintln!("skipping: reference artifact not built");
        return None;
    }
    let bytes = std::fs::read(REF_ARTIFACT).expect("read reference artifact");
    let f = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(f.path(), &bytes).unwrap();
    Some(f)
}

fn ref_mock(headers: Vec<(&str, &str)>) -> MockPluginSession {
    let mut s = MockPluginSession::new();
    let owned: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    s.expect_request_headers().returning(move || owned.clone());
    let hmap: std::collections::HashMap<String, String> =
        headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    s.expect_header_value().returning(move |name| hmap.get(name).cloned());
    s.expect_get_path().return_const("/x".to_string());
    s.expect_get_method().return_const("GET".to_string());
    s.expect_remote_ip().return_const("1.2.3.4".to_string());
    // A guest deny path (send_http_response) applies local-response headers.
    s.expect_set_response_header().returning(|_, _| Ok(()));
    // The proxy-wasm Rust SDK exports `proxy_on_request_body` on every
    // guest, so the request-body phase now runs for the reference module.
    // These tests exercise the UNGATED plugin (no `body:` requirement),
    // where `read_request_body` returns `Unimplemented`, which now routes
    // through `fail_mode` (Task 7) rather than being skipped. Every test
    // function here that actually reaches this arm sets `c.fail_open =
    // true` (with its own comment) so the phase stays a no-op GoodNext.
    s.expect_ctx().return_const(crate::EdgionHttpContext::new());
    s.expect_read_request_body()
        .returning(|_| Err(crate::plugins::runtime::traits::session::BodyReadError::Unimplemented));
    s
}

/// Real SDK guest: callout to an auth endpoint that returns 200 for a valid
/// bearer token → guest resumes and applies `x-wasm-added`. Also asserts the
/// forwarded Authorization header reached the callout target.
#[tokio::test]
async fn reference_guest_callout_authorized_resumes() {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let Some(f) = ref_artifact_temp() else { return };
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/verify"))
        .and(header("authorization", "Bearer valid-token"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let mut c = WasmConfig::default();
    c.source.file = Some(f.path().to_string_lossy().into_owned());
    c.callout_allowlist = vec![server.uri()];
    // Not testing body-read policy here; the SDK guest also exports
    // `proxy_on_request_body`, whose `Unimplemented` stub now runs through
    // `fail_mode` (Task 7), so `fail_open` keeps that phase a no-op.
    c.fail_open = true;

    let plugin = actual_request(&c).unwrap();
    let mut session = ref_mock(vec![
        ("host", "t.example.com"),
        ("x-wasm-callout-url", &server.uri()),
        ("authorization", "Bearer valid-token"),
    ]);
    session
        .expect_set_request_header()
        .withf(|k, v| k == "x-wasm-added" && v == "yes")
        .times(1)
        .returning(|_, _| Ok(()));
    session.expect_record_outcome().return_const(());
    // See `reference_guest_module_loads_and_runs` for why this fires (R17).
    session.expect_record_error().return_const(());
    let mut log = PluginLog::new("Wasm");
    let res = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(res, PluginRunningResult::GoodNext, "authorized callout must resume");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

/// Real SDK guest: callout returns 401 for a bad token → guest denies with
/// a 403 local response.
#[tokio::test]
async fn reference_guest_callout_unauthorized_denies() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let Some(f) = ref_artifact_temp() else { return };
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/verify"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let mut c = WasmConfig::default();
    c.source.file = Some(f.path().to_string_lossy().into_owned());
    c.callout_allowlist = vec![server.uri()];

    let plugin = actual_request(&c).unwrap();
    let mut session = ref_mock(vec![
        ("host", "t.example.com"),
        ("x-wasm-callout-url", &server.uri()),
        ("authorization", "Bearer nope"),
    ]);
    let mut log = PluginLog::new("Wasm");
    let res = plugin.run_request(&mut session, &mut log).await;
    assert!(
        matches!(res, PluginRunningResult::ErrResponse { status: 403, .. }),
        "unauthorized callout must deny, got {res:?}"
    );
}

/// Real SDK guest: `on_tick` increments a shared counter; a later request
/// reads it into `x-wasm-ticks`. Proves the background tick task runs and
/// shared data is visible to request hooks. (Guest tick period is 1s.)
#[tokio::test]
async fn reference_guest_tick_increments_shared_counter() {
    let Some(f) = ref_artifact_temp() else { return };
    let mut c = WasmConfig::default();
    c.source.file = Some(f.path().to_string_lossy().into_owned());
    // Not testing body-read policy here; the SDK guest also exports
    // `proxy_on_request_body`, whose `Unimplemented` stub now runs through
    // `fail_mode` (Task 7), so `fail_open` keeps that phase a no-op.
    c.fail_open = true;

    // Load warms one instance, which runs on_configure (sets the 1s tick)
    // and arms the module tick task.
    let plugin = actual_plugin(&c);
    plugin.runtime.get_or_load().await.expect("module loads");
    // Wait for at least two ticks.
    tokio::time::sleep(std::time::Duration::from_millis(2300)).await;

    let seen = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let seen2 = seen.clone();
    let mut session = ref_mock(vec![("host", "t.example.com"), ("x-wasm-read-ticks", "1")]);
    session
        .expect_set_request_header()
        .withf(move |k, v| {
            if k == "x-wasm-ticks" {
                seen2.store(v.parse().unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
            }
            true
        })
        .returning(|_, _| Ok(()));
    session.expect_record_outcome().return_const(());
    // See `reference_guest_module_loads_and_runs` for why this fires (R17).
    session.expect_record_error().return_const(());
    let mut log = PluginLog::new("Wasm");
    let res = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(res, PluginRunningResult::GoodNext);
    assert!(
        seen.load(std::sync::atomic::Ordering::Relaxed) >= 1,
        "on_tick must have fired at least once and be readable via shared data"
    );
}

/// Real SDK guest: with `pluginConfig` set to a callout origin, `on_tick`
/// dispatches a GET /verify callout; the 200 callback (delivered on the
/// root context by the tick task) bumps the shared `calloutticks`
/// counter, which a later request reads into `x-wasm-calloutticks`.
/// End-to-end proof of the tick-callout loop with the reference guest.
#[tokio::test]
async fn reference_guest_tick_callout_polls_and_publishes() {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let Some(f) = ref_artifact_temp() else { return };
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/verify"))
        .and(header("authorization", "Bearer valid-token"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let mut c = WasmConfig::default();
    c.source.file = Some(f.path().to_string_lossy().into_owned());
    c.plugin_config = Some(WasmRawConfig::string(server.uri()));
    c.callout_allowlist = vec![server.uri()];
    // Not testing body-read policy here; the SDK guest also exports
    // `proxy_on_request_body`, whose `Unimplemented` stub now runs through
    // `fail_mode` (Task 7), so `fail_open` keeps that phase a no-op.
    c.fail_open = true;

    // Building the plugin binds its callout grant and preloads its
    // runtime; the eager warm instance runs on_configure (1s tick).
    let plugin = actual_request(&c).unwrap();

    // Poll until a tick's callout has been published, rather than sleeping a fixed 2.5s
    // and asserting once. The tick is 1s and each one does an HTTP callout round-trip, so
    // a fixed wait is a bet on wall-clock scheduling: under a full `cargo test --all` the
    // machine is saturated and 2.5s can pass without two ticks landing, which failed this
    // assertion for a reason that has nothing to do with what it is testing. Polling to a
    // generous deadline keeps the test fast on an idle machine and honest on a busy one.
    let seen = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut res = None;
    while std::time::Instant::now() < deadline {
        let seen2 = seen.clone();
        let mut session = ref_mock(vec![("host", "t.example.com"), ("x-wasm-read-callout", "1")]);
        session
            .expect_set_request_header()
            .withf(move |k, v| {
                if k == "x-wasm-calloutticks" {
                    seen2.store(v.parse().unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
                }
                true
            })
            .returning(|_, _| Ok(()));
        session.expect_record_outcome().return_const(());
        // See `reference_guest_module_loads_and_runs` for why this fires (R17).
        session.expect_record_error().return_const(());
        let mut log = PluginLog::new("Wasm");
        res = Some(plugin.run_request(&mut session, &mut log).await);
        if seen.load(std::sync::atomic::Ordering::Relaxed) >= 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert_eq!(res, Some(PluginRunningResult::GoodNext));
    assert!(
        seen.load(std::sync::atomic::Ordering::Relaxed) >= 1,
        "tick callouts must have completed and been published via shared data"
    );
    assert!(
        !server.received_requests().await.unwrap().is_empty(),
        "the tick callout must reach the target"
    );
}

/// Real SDK guest: the read-only body hook adds each chunk's byte count to
/// a shared counter; a later request reads it into `x-wasm-bodybytes`.
/// Ties body observation → shared data → cross-request read.
#[tokio::test]
async fn reference_guest_body_observation_accumulates_bytes() {
    let Some(f) = ref_artifact_temp() else { return };
    let mut c = WasmConfig::default();
    c.source.file = Some(f.path().to_string_lossy().into_owned());
    // Not testing request body-read policy here; the SDK guest also
    // exports `proxy_on_request_body`, whose `Unimplemented` stub now runs
    // through `fail_mode` (Task 7), so `fail_open` keeps that phase a
    // no-op for the request below (this test's own body path goes through
    // the response body filter, not `read_request_body`).
    c.fail_open = true;
    let plugin = actual_plugin(&c);
    plugin.runtime.get_or_load().await.expect("module loads");

    // Drive two response-body chunks through the body filter.
    let mut bsession = MockPluginSession::new();
    let chunk = Some(bytes::Bytes::from_static(&[b'x'; 100]));
    assert!(plugin
        .run_upstream_response_body_filter(&chunk, false, &mut bsession)
        .is_none());
    assert!(plugin
        .run_upstream_response_body_filter(&chunk, true, &mut bsession)
        .is_none());

    // A request on the same plugin reads its accumulated byte count.
    let seen = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let seen2 = seen.clone();
    let mut session = ref_mock(vec![("host", "t.example.com"), ("x-wasm-read-body", "1")]);
    session
        .expect_set_request_header()
        .withf(move |k, v| {
            if k == "x-wasm-bodybytes" {
                seen2.store(v.parse().unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
            }
            true
        })
        .returning(|_, _| Ok(()));
    session.expect_record_outcome().return_const(());
    // See `reference_guest_module_loads_and_runs` for why this fires (R17).
    session.expect_record_error().return_const(());
    let mut log = PluginLog::new("Wasm");
    let res = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(res, PluginRunningResult::GoodNext);
    assert!(
        seen.load(std::sync::atomic::Ordering::Relaxed) >= 200,
        "body hook must have counted both 100-byte chunks into shared data"
    );
}

/// Request-body buffer-mode guest: `proxy_on_request_body` sends a 403
/// local response when the buffered body is non-empty (a WAF/scanner deny),
/// otherwise continues. Also exports a no-op `proxy_on_request_headers` so
/// the request-headers phase runs to GoodNext and the body phase is reached.
const GUEST_REQUEST_BODY_DENY: &str = r#"
    (module
      (import "env" "proxy_send_local_response"
        (func $deny (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
      (func (export "proxy_on_request_body") (param i32 i32 i32) (result i32)
        (if (i32.gt_s (local.get 1) (i32.const 0))
          (then (drop (call $deny (i32.const 403) (i32.const 0) (i32.const 0)
                                  (i32.const 0) (i32.const 0) (i32.const 0)
                                  (i32.const 0) (i32.const 0)))))
        (i32.const 0)))
    "#;

const GUEST_REQUEST_BODY_REWRITE: &str = r#"
    (module
      (import "env" "proxy_set_buffer_bytes"
        (func $set (param i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_replace_header_map_value"
        (func $set_hdr (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 0) "XYZ")
      (data (i32.const 16) "x-body-rewritten")
      (data (i32.const 40) "yes")
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
      (func (export "proxy_on_request_body") (param i32 i32 i32) (result i32)
        (drop (call $set (i32.const 0) (i32.const 1) (i32.const 2)
                         (i32.const 0) (i32.const 3)))
        (drop (call $set_hdr (i32.const 0) (i32.const 16) (i32.const 16)
                             (i32.const 40) (i32.const 3)))
        (i32.const 0)))
    "#;

const GUEST_REQUEST_BODY_REWRITE_THEN_DENY: &str = r#"
    (module
      (import "env" "proxy_set_buffer_bytes"
        (func $set (param i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_send_local_response"
        (func $deny (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 0) "XYZ")
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
      (func (export "proxy_on_request_body") (param i32 i32 i32) (result i32)
        (drop (call $set (i32.const 0) (i32.const 1) (i32.const 2)
                         (i32.const 0) (i32.const 3)))
        (drop (call $deny (i32.const 403) (i32.const 0) (i32.const 0)
                          (i32.const 0) (i32.const 0) (i32.const 0)
                          (i32.const 0) (i32.const 0)))
        (i32.const 0)))
    "#;

const GUEST_REQUEST_BODY_REWRITE_THEN_TRAP: &str = r#"
    (module
      (import "env" "proxy_set_buffer_bytes"
        (func $set (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 0) "XYZ")
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
      (func (export "proxy_on_request_body") (param i32 i32 i32) (result i32)
        (drop (call $set (i32.const 0) (i32.const 1) (i32.const 2)
                         (i32.const 0) (i32.const 3)))
        unreachable))
    "#;

/// Request-phase mock with a ctx (for the body-size cap resolution) and a
/// single scripted `read_request_body` outcome. Built standalone rather
/// than on `base_mock` so this is the only `read_request_body` expectation
/// (mockall matches FIFO; `base_mock`'s unlimited stub would otherwise win).
fn body_mock(read: crate::plugins::runtime::traits::session::BodyReadResult) -> MockPluginSession {
    let mut s = MockPluginSession::new();
    s.expect_request_headers()
        .returning(|| vec![("host".to_string(), "t.example.com".to_string())]);
    s.expect_get_path().return_const("/x".to_string());
    s.expect_get_method().return_const("GET".to_string());
    s.expect_header_value().returning(|_| None);
    s.expect_remote_ip().return_const("1.2.3.4".to_string());
    s.expect_ctx().return_const(crate::EdgionHttpContext::new());
    s.expect_read_request_body().return_once(move |_| read);
    s
}

/// A guest with a request-body hook that denies on a non-empty buffered
/// body must short-circuit with the guest's local response (403) once the
/// body gate has buffered the body (`read_request_body` -> Ok).
#[tokio::test]
async fn request_body_guest_receives_full_body_and_can_deny() {
    let f = write_temp_wasm(GUEST_REQUEST_BODY_DENY);
    let plugin = actual_request(&config_for(f.path(), false)).expect("plugin");
    let mut session = body_mock(Ok(bytes::Bytes::from_static(b"scan-me")));
    let mut log = PluginLog::new("Wasm");
    let res = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(
        res,
        PluginRunningResult::ErrResponse {
            status: 403,
            body: None
        }
    );
}

#[tokio::test]
async fn request_body_guest_commits_rewrite_to_shared_body() {
    let f = write_temp_wasm(GUEST_REQUEST_BODY_REWRITE);
    let plugin = actual_request(&config_for(f.path(), false)).expect("plugin");
    let mut session = body_mock(Ok(bytes::Bytes::from_static(b"abcdef")));
    session
        .expect_overwrite_request_body()
        .withf(|body| body.as_ref() == b"aXYZdef")
        .times(1)
        .returning(|_| Ok(()));
    session
        .expect_set_request_header()
        .withf(|name, value| name == "x-body-rewritten" && value == "yes")
        .times(1)
        .returning(|_, _| Ok(()));
    let mut log = PluginLog::new("Wasm");

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn request_body_local_response_skips_irrelevant_rewrite_publication() {
    let f = write_temp_wasm(GUEST_REQUEST_BODY_REWRITE_THEN_DENY);
    let plugin = actual_request(&config_for(f.path(), false)).expect("plugin");
    let mut session = body_mock(Ok(bytes::Bytes::from_static(b"abcdef")));
    let mut log = PluginLog::new("Wasm");

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse {
            status: 403,
            body: None,
        },
        "a BodyStorage write must not replace a deliberate local denial"
    );
}

#[tokio::test]
async fn request_body_trap_discards_staged_rewrite() {
    let f = write_temp_wasm(GUEST_REQUEST_BODY_REWRITE_THEN_TRAP);
    let plugin = actual_request(&config_for(f.path(), false)).expect("plugin");
    let mut session = body_mock(Ok(bytes::Bytes::from_static(b"abcdef")));
    session.expect_overwrite_request_body().times(0);
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "trap")
        .times(1)
        .return_const(());
    let mut log = PluginLog::new("Wasm");

    assert!(matches!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse { status: 500, .. }
    ));
}

#[tokio::test]
async fn request_body_publication_failure_is_fail_closed_and_observable() {
    let f = write_temp_wasm(GUEST_REQUEST_BODY_REWRITE);
    let plugin = actual_request(&config_for(f.path(), false)).expect("plugin");
    let mut session = body_mock(Ok(bytes::Bytes::from_static(b"abcdef")));
    session
        .expect_overwrite_request_body()
        .withf(|body| body.as_ref() == b"aXYZdef")
        .times(1)
        .returning(|_| Err(BodyReadError::StorageFailure));
    session.expect_set_request_header().times(0);
    session
        .expect_record_error()
        .withf(|status| *status == edgion_resources::EdgionErrorCode::BodyCaptureStorageFailed)
        .times(1)
        .return_const(());
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "body-mutation")
        .times(1)
        .return_const(());
    let mut log = PluginLog::new("Wasm");

    assert!(matches!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse { status: 500, .. }
    ));
}

#[tokio::test]
async fn request_body_publication_fail_open_leaks_no_partial_header_commit() {
    let f = write_temp_wasm(GUEST_REQUEST_BODY_REWRITE);
    let plugin = actual_request(&config_for(f.path(), true)).expect("plugin");
    let mut session = body_mock(Ok(bytes::Bytes::from_static(b"abcdef")));
    session
        .expect_overwrite_request_body()
        .times(1)
        .returning(|_| Err(BodyReadError::StorageFailure));
    session.expect_set_request_header().times(0);
    session
        .expect_record_error()
        .withf(|status| *status == edgion_resources::EdgionErrorCode::BodyCaptureStorageFailed)
        .times(1)
        .return_const(());
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "body-mutation")
        .times(1)
        .return_const(());
    let mut log = PluginLog::new("Wasm");

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

/// When `read_request_body` returns `Unimplemented` (ungated plugin / no
/// buffering backing), the deny guest's body hook is not driven — but this
/// is a genuine read failure, not admission, so it goes through
/// `fail_mode` like any other Wasm runtime failure (Task 7). With the
/// `fail_open: false` default that means fail-closed (500), not a silent
/// GoodNext skip; `wasm_read_failure_honors_fail_open` pins the
/// `fail_open: true` override of this same read error.
#[tokio::test]
async fn request_body_read_failure_fails_closed_by_default() {
    use crate::plugins::runtime::traits::session::BodyReadError;
    let f = write_temp_wasm(GUEST_REQUEST_BODY_DENY);
    let plugin = actual_request(&config_for(f.path(), false)).expect("plugin");
    let mut session = body_mock(Err(BodyReadError::Unimplemented));
    session
        .expect_record_error()
        .withf(|status| *status == edgion_resources::EdgionErrorCode::BodyReadUnimplemented)
        .times(1)
        .return_const(());
    session.expect_record_outcome().return_const(());
    let mut log = PluginLog::new("Wasm");
    let res = plugin.run_request(&mut session, &mut log).await;
    assert!(matches!(res, PluginRunningResult::ErrResponse { status: 500, .. }));
}

/// R17: an ungated wasm module (no operator `body:` block) whose body read
/// fails with `Unimplemented` must still record an `EdgionErrorCode` via
/// `ctx.record_error`, not just a `PluginLog` line — otherwise the newly-common
/// default-500 for a body-hook wasm guest is invisible in the access log's
/// structured diagnostics array. Pins the mapping used at the
/// `run_request_body_phase` call site (mirrors `body_gate.rs`'s
/// `body_read_failure_result`).
#[tokio::test]
async fn ungated_body_read_unimplemented_records_error_diagnostic() {
    let f = write_temp_wasm(GUEST_REQUEST_BODY_DENY);
    let plugin = actual_request(&config_for(f.path(), false)).expect("plugin");
    let mut session = body_mock(Err(BodyReadError::Unimplemented));
    session
        .expect_record_error()
        .withf(|status| *status == edgion_resources::EdgionErrorCode::BodyReadUnimplemented)
        .times(1)
        .return_const(());
    session.expect_record_outcome().return_const(());
    let mut log = PluginLog::new("Wasm");
    let res = plugin.run_request(&mut session, &mut log).await;
    assert!(
        matches!(res, PluginRunningResult::ErrResponse { status: 500, .. }),
        "fail_open defaults to false, so the ungated read failure must still fail-close"
    );
}

/// R5: `failOpen: true` governs wasm RUNTIME failures, not admission. A refusal
/// means the guest's body hook would inspect nothing, so it must fail closed
/// even though this plugin is configured fail-open.
#[tokio::test]
async fn wasm_refusal_is_fail_close_even_with_fail_open_configured() {
    use crate::plugins::runtime::traits::session::BodyReadError;
    let f = write_temp_wasm(GUEST_REQUEST_BODY_DENY);
    let plugin = actual_request(&config_for(f.path(), true)).expect("plugin");
    let mut session = body_mock(Err(BodyReadError::NotBuffered));
    let mut log = PluginLog::new("Wasm");

    let res = plugin.run_request(&mut session, &mut log).await;
    assert!(
        matches!(res, PluginRunningResult::ErrResponse { status: 500, .. }),
        "a refusal must not be softened by failOpen, got {res:?}"
    );
}

/// R6: a genuine read failure keeps the existing knob. With `failOpen: true`
/// an `Unimplemented` read leaves the body hook undriven and the request
/// continues — the fail-close default for this same read error is pinned by
/// `request_body_read_failure_fails_closed_by_default`.
#[tokio::test]
async fn wasm_read_failure_honors_fail_open() {
    use crate::plugins::runtime::traits::session::BodyReadError;
    let f = write_temp_wasm(GUEST_REQUEST_BODY_DENY);
    let plugin = actual_request(&config_for(f.path(), true)).expect("plugin");
    let mut session = body_mock(Err(BodyReadError::Unimplemented));
    session
        .expect_record_error()
        .withf(|status| *status == edgion_resources::EdgionErrorCode::BodyReadUnimplemented)
        .times(1)
        .return_const(());
    session.expect_record_outcome().return_const(());
    let mut log = PluginLog::new("Wasm");

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

/// Request-body buffer-mode guest that reads the `host` request header
/// (via `proxy_get_header_map_value` against `MAP_HTTP_REQUEST_HEADERS`)
/// from inside `proxy_on_request_body`, and denies with 403 only when
/// BOTH the header lookup succeeds (`STATUS_OK` == 0) AND the buffered
/// body is non-empty. This proves the body phase snapshot carries the
/// real request headers (regression guest for the "empty headers in
/// request-body phase" bug — before the fix, `get_hdr` would return
/// `STATUS_NOT_FOUND` and the guest would never deny even with a body).
const GUEST_REQUEST_BODY_READS_HEADER: &str = r#"
    (module
      (import "env" "proxy_get_header_map_value"
        (func $get_hdr (param i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_send_local_response"
        (func $deny (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (global $heap (mut i32) (i32.const 8192))
      (data (i32.const 0) "host")
      (func (export "proxy_on_memory_allocate") (param i32) (result i32)
        (local $p i32)
        global.get $heap
        local.set $p
        global.get $heap
        local.get 0
        i32.add
        global.set $heap
        local.get $p)
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
      (func (export "proxy_on_request_body") (param i32 i32 i32) (result i32)
        (if (i32.and
              (i32.eqz (call $get_hdr (i32.const 0) (i32.const 0) (i32.const 4)
                                      (i32.const 256) (i32.const 260)))
              (i32.gt_s (local.get 1) (i32.const 0)))
          (then (drop (call $deny (i32.const 403) (i32.const 0) (i32.const 0)
                                  (i32.const 0) (i32.const 0) (i32.const 0)
                                  (i32.const 0) (i32.const 0)))))
        (i32.const 0)))
    "#;

/// Regression for the request-body phase snapshotting empty request
/// headers: a guest that reads the `host` header AND the body inside
/// `proxy_on_request_body` must see both — the header lookup succeeds and
/// the buffered body is non-empty, so the guest denies with 403. Before
/// the fix, `begin_phase(WasmPhase::RequestBody, ...)` was seeded with an
/// empty header vector, so `proxy_get_header_map_value` would return
/// `STATUS_NOT_FOUND` and this guest would never deny.
#[tokio::test]
async fn request_body_guest_can_read_request_header_and_body() {
    let f = write_temp_wasm(GUEST_REQUEST_BODY_READS_HEADER);
    let plugin = actual_request(&config_for(f.path(), false)).expect("plugin");
    let mut session = body_mock(Ok(bytes::Bytes::from_static(b"scan-me")));
    let mut log = PluginLog::new("Wasm");
    let res = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(
        res,
        PluginRunningResult::ErrResponse {
            status: 403,
            body: None
        },
        "guest must see the request header AND the body in the body phase"
    );
}

/// `BodyGatedRequestFilter` (the operator `body:` gate) buffers the
/// request body once to enforce its own read-failure/size policy, then
/// hands off to the wrapped `WasmPlugin`, whose own request-body phase
/// re-reads the SAME session via `read_request_body` a second time (see
/// `run_request_body_phase`). `PluginSession::read_request_body` must be
/// idempotent (cached bytes served on every call), so this mock uses
/// `.returning(...)` rather than `.return_once(...)` to prove both the
/// gate and the inner Wasm body phase can call it and both see the body.
#[tokio::test]
async fn body_gate_and_wasm_body_phase_both_read_request_body() {
    let f = write_temp_wasm(GUEST_REQUEST_BODY_DENY);
    let plugin = actual_request(&config_for(f.path(), false)).expect("plugin");
    let gate = crate::plugins::runtime::BodyGatedRequestFilter::new(
        plugin,
        edgion_resources::resources::edgion_plugins::BodyRequirement::fail_close(),
    );

    let mut session = MockPluginSession::new();
    session
        .expect_request_headers()
        .returning(|| vec![("host".to_string(), "t.example.com".to_string())]);
    session.expect_get_path().return_const("/x".to_string());
    session.expect_get_method().return_const("GET".to_string());
    session.expect_header_value().returning(|_| None);
    session.expect_remote_ip().return_const("1.2.3.4".to_string());
    session.expect_ctx().return_const(crate::EdgionHttpContext::new());
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls2 = calls.clone();
    session.expect_read_request_body().returning(move |_| {
        calls2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(bytes::Bytes::from_static(b"scan-me"))
    });

    let mut log = PluginLog::new("Wasm");
    let res = gate.run_request(&mut session, &mut log).await;
    // The guest hook (GUEST_REQUEST_BODY_DENY) receives the buffered body
    // in `proxy_on_request_body` and denies on the non-empty body.
    assert_eq!(
        res,
        PluginRunningResult::ErrResponse {
            status: 403,
            body: None
        }
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the gate and the wasm body phase must both read the request body"
    );
}

#[test]
fn apply_ops_replace_all_removes_omitted_request_headers() {
    // ReplaceAll must be a true replacement: a name present before the
    // replace (authorization) but absent from the new vector must be
    // removed, not left to leak downstream as an additive set.
    let ops = vec![HeaderOp::ReplaceAll {
        map: MAP_HTTP_REQUEST_HEADERS,
        remove: vec!["authorization".to_string(), "x-keep".to_string()],
        pairs: vec![("x-keep".to_string(), "new".to_string())],
    }];
    let mut session = MockPluginSession::new();
    session
        .expect_remove_request_header()
        .withf(|n| n == "authorization")
        .times(1)
        .returning(|_| Ok(()));
    session
        .expect_remove_request_header()
        .withf(|n| n == "x-keep")
        .times(1)
        .returning(|_| Ok(()));
    session
        .expect_set_request_header()
        .withf(|n, v| n == "x-keep" && v == "new")
        .times(1)
        .returning(|_, _| Ok(()));
    apply_ops(&mut session, ops);
}

#[test]
fn apply_ops_replace_all_preserves_duplicate_response_values() {
    // Repeated names in the replacement (e.g. multiple Set-Cookie) must
    // stay distinct: first occurrence set (replace-all), later occurrences
    // appended, instead of collapsing to the last value.
    let ops = vec![HeaderOp::ReplaceAll {
        map: MAP_HTTP_RESPONSE_HEADERS,
        remove: vec![
            "set-cookie".to_string(),
            "Content-Length".to_string(),
            "Upgrade".to_string(),
        ],
        pairs: vec![
            ("CONTENT-LENGTH".to_string(), "999".to_string()),
            ("Upgrade".to_string(), "spoofed".to_string()),
            ("set-cookie".to_string(), "a=1".to_string()),
            ("set-cookie".to_string(), "b=2".to_string()),
        ],
    }];
    let mut session = MockPluginSession::new();
    session
        .expect_remove_response_header()
        .withf(|n| n == "set-cookie")
        .times(1)
        .returning(|_| Ok(()));
    session
        .expect_set_response_header()
        .withf(|n, v| n == "set-cookie" && v == "a=1")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_append_response_header()
        .withf(|n, v| n == "set-cookie" && v == "b=2")
        .times(1)
        .returning(|_, _| Ok(()));
    apply_ops(&mut session, ops);
}

#[tokio::test]
async fn response_policy_replace_all_and_individual_ops_preserve_transport_fields() {
    use crate::plugins::runtime::session_adapter::record_outcome_tests::test_session;
    use crate::plugins::runtime::PingoraSessionAdapter;
    use crate::EdgionHttpContext;
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    let mut response = pingora_http::ResponseHeader::build(101, None).unwrap();
    for (name, value) in [("content-length", "2"), ("upgrade", "websocket"), ("x-old", "old")] {
        response.insert_header(name, value).unwrap();
    }
    {
        let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response);
        apply_ops(
            &mut adapter,
            vec![
                HeaderOp::Set {
                    map: MAP_HTTP_RESPONSE_HEADERS,
                    name: "Content-Length".into(),
                    value: "999".into(),
                },
                HeaderOp::Add {
                    map: MAP_HTTP_RESPONSE_HEADERS,
                    name: "Upgrade".into(),
                    value: "spoofed".into(),
                },
                HeaderOp::Remove {
                    map: MAP_HTTP_RESPONSE_HEADERS,
                    name: "Content-Length".into(),
                },
                HeaderOp::ReplaceAll {
                    map: MAP_HTTP_RESPONSE_HEADERS,
                    remove: vec!["content-length".into(), "upgrade".into(), "x-old".into()],
                    pairs: vec![
                        ("content-length".into(), "999".into()),
                        ("Set-Cookie".into(), "a=1".into()),
                        ("set-cookie".into(), "b=2".into()),
                        ("Content-Type".into(), "text/plain".into()),
                    ],
                },
            ],
        );
    }
    assert_eq!(response.headers["content-length"], "2");
    assert_eq!(response.headers["upgrade"], "websocket");
    assert!(!response.headers.contains_key("x-old"));
    assert_eq!(response.headers["content-type"], "text/plain");
    assert_eq!(
        response
            .headers
            .get_all("set-cookie")
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect::<Vec<_>>(),
        ["a=1", "b=2"]
    );
}

fn wasm_owner(config: &WasmConfig) -> edgion_resources::EdgionPlugins {
    serde_json::from_value(serde_json::json!({
        "metadata": {"name": "wasm-test", "namespace": "default"},
        "spec": {"requestPlugins": [{"type": "Wasm", "config": config}]}
    }))
    .unwrap()
}

fn actual_plugin(config: &WasmConfig) -> WasmPlugin {
    let owner = wasm_owner(config);
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    WasmPlugin::build(
        &entry,
        &edgion_common::link_sys_core::outbound_tls::outbound_global_default(),
    )
    .unwrap()
}
fn actual_request(config: &WasmConfig) -> Option<Box<dyn RequestFilter>> {
    Some(Box::new(actual_plugin(config)))
}

#[tokio::test]
async fn wasm_bound_crypto_load_failure_preserves_both_failure_modes() {
    for fail_open in [false, true] {
        let (mut owner, _, _) = super::super::vm_manager::tests::wasm_tls_owner();
        let EdgionPlugin::Wasm(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin else {
            panic!()
        };
        config.fail_open = fail_open;
        config
            .source
            .fetch
            .as_mut()
            .unwrap()
            .tls
            .as_mut()
            .unwrap()
            .resolved_ca_certificates
            .as_mut()
            .unwrap()[0]
            .data
            .as_mut()
            .unwrap()
            .insert("ca.crt".into(), k8s_openapi::ByteString(b"invalid PEM".to_vec()));
        let defaults = edgion_common::link_sys_core::outbound_tls::test_util::snapshot(Default::default());
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let entry = binding.request_entries().next().unwrap();
        let plugin = WasmPlugin::build(&entry, &defaults).unwrap();
        let mut session = base_mock();
        session
            .expect_record_outcome()
            .withf(move |outcome, _| {
                if fail_open {
                    matches!(outcome, DyeOutcome::FailOpen)
                } else {
                    matches!(outcome, DyeOutcome::FailClose)
                }
            })
            .times(1)
            .return_const(());
        let mut log = PluginLog::new("Wasm");
        let result = plugin.run_request(&mut session, &mut log).await;
        if fail_open {
            assert_eq!(result, PluginRunningResult::GoodNext);
        } else {
            assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
        }
    }
}
