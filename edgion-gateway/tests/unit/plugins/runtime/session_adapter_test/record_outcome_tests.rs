use super::*;
use crate::plugins::http::dsl::lang::vm::{Vm, VmLimits};
use crate::plugins::http::dsl::runtime::DslRuntime;
use edgion_dsl::{error::RuntimeError, resolve_script, validator::ValidationLimits};

fn dsl_body_test_vm(source: &str) -> Vm {
    let script = resolve_script(Some(source), None, &ValidationLimits::default()).unwrap();
    Vm::new(script, VmLimits::default()).unwrap()
}

#[tokio::test]
async fn dsl_body_aliases_follow_mutations_while_transfer_reader_stays_frozen() {
    let vm = dsl_body_test_vm(
        r#"
                let first = req.body
                let alias = first
                let original = first.sha256()
                first.append("d")
                if alias.read(0, alias.byte_len()) != "abcd" || alias.sha256() == original {
                    return deny(400, "alias did not observe append")
                }
                alias.remove(1, 3)
                first.prepend("x")
                alias.replace("a", "A")
                if first.read(0, first.byte_len()) != "xAd" {
                    return deny(400, "alias did not observe mutation")
                }
                first.clear()
                if alias.byte_len() != 0 || alias.read(0, alias.byte_len()) != "" {
                    return deny(400, "alias did not observe clear")
                }
                alias.append("z")
                if first.read(0, first.byte_len()) != "z" {
                    return deny(400, "alias did not observe new generation")
                }
            "#,
    );
    for memory_limit in [64, 1] {
        let mut session = test_session_with_body(b"abc").await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = true;
        ctx.request_body_config.memory_buffer_bytes = memory_limit;
        ctx.request_body_config.max_body_size_bytes = 64;

        let mut frozen = {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            assert_eq!(adapter.ensure_request_body(None).await.unwrap(), 3);
            let reader = adapter.request_body_snapshot_reader(false).await.unwrap();
            let result = vm
                .execute(&mut adapter, &mut PluginLog::new("test"), &DslRuntime::default())
                .await
                .unwrap();
            assert_eq!(result, PluginRunningResult::GoodNext);
            reader
        };
        assert_eq!(frozen.next_chunk().await.unwrap(), Some(Bytes::from_static(b"abc")));
        assert_eq!(frozen.next_chunk().await.unwrap(), None);
        assert_eq!(ctx.request_body.generation(), Some(6));
        assert_eq!(ctx.request_body.modified_len_for_framing().unwrap(), Some(1));
        assert_eq!(
            ctx.request_body.read_range(0, 64, 64).await.unwrap().value,
            Bytes::from_static(b"z")
        );
    }
}

#[tokio::test]
async fn dsl_body_response_stage_reads_cache_but_refuses_every_mutation() {
    let read_vm = dsl_body_test_vm(
        r#"
                if req.body.read(0, req.body.byte_len()) != "abc" || req.body.read(1, 1) != "b" {
                    return deny(400, "late read mismatch")
                }
            "#,
    );
    for memory_limit in [64, 1] {
        let mut session = test_session_with_body(b"abc").await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = true;
        ctx.request_body_config.memory_buffer_bytes = memory_limit;
        {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            adapter.ensure_request_body(None).await.unwrap();
        }
        ctx.request_info.in_request_body_stage = false;
        for source in [
            "req.body.clear()",
            r#"req.body.append("x")"#,
            r#"req.body.prepend("x")"#,
            "req.body.remove(0, 1)",
            r#"req.body.replace("a", "x")"#,
        ] {
            // Bypass stage-specific config rejection to exercise the host
            // boundary even for a directly constructed VM.
            let mutation_vm = dsl_body_test_vm(source);
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            assert_eq!(
                read_vm
                    .execute(&mut adapter, &mut PluginLog::new("test"), &DslRuntime::default())
                    .await
                    .unwrap(),
                PluginRunningResult::GoodNext
            );
            assert!(matches!(
                mutation_vm
                    .execute(&mut adapter, &mut PluginLog::new("test"), &DslRuntime::default())
                    .await,
                Err(RuntimeError::BodyAccess { .. })
            ));
        }
        assert_eq!(ctx.request_body.generation(), Some(0));
        assert_eq!(ctx.request_body.modified_len_for_framing().unwrap(), None);
        assert_eq!(
            ctx.request_body.read_range(0, 64, 64).await.unwrap().value,
            Bytes::from_static(b"abc")
        );
    }
}

#[tokio::test]
async fn dsl_body_late_read_never_captures_and_proven_empty_needs_no_cache() {
    let vm = dsl_body_test_vm(
        r#"
                if req.body.read(0, req.body.byte_len()) != "" || req.body.read(0, 100) != "" {
                    return deny(400, "expected empty body")
                }
            "#,
    );
    for declared in [Some(true), None, Some(false)] {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = declared;
        ctx.request_info.in_request_body_stage = false;
        let result = {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            vm.execute(&mut adapter, &mut PluginLog::new("test"), &DslRuntime::default())
                .await
        };
        if declared == Some(false) {
            assert_eq!(result.unwrap(), PluginRunningResult::GoodNext);
        } else {
            assert!(matches!(result, Err(RuntimeError::BodyAccess { .. })));
        }
        assert!(!ctx.request_body.has_cached_body());
    }
}
use crate::plugins::http::RequestBodyBuffer as RequestBodyBufferPlugin;
use crate::plugins::runtime::{BodyGatedRequestFilter, RequestFilter};
use crate::request_body::DEFAULT_MEMORY_BUFFER_BYTES;
use edgion_resources::resources::edgion_plugins::BodyRequirement;
use edgion_resources::resources::edgion_plugins::{DyeOutcome, DyeSuite};
use pingora_proxy::RequestBodyEvent;
use sha2::Digest;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

/// Build a minimal real `pingora_proxy::Session` for adapter-level tests.
///
/// `record_outcome`'s real implementation lives on `PingoraSessionAdapter`
/// (not a generic `&dyn PluginSession` free function), and constructing
/// the adapter needs a live `&mut Session` field even for a test that
/// never touches the downstream request. Nothing else in this codebase
/// builds one (see `dye_strip_tests`'s comment above, which sidesteps the
/// same gap by testing a pure core function instead) — but pingora's own
/// `Session::new_h1` doc comment says it "is mostly used for testing and
/// mocking", and `tokio::io::DuplexStream` implements pingora's `IO`
/// trait for exactly this purpose
/// (`pingora_core::protocols::ext_io_impl`). A `Session` needs a request
/// already read off the wire before any other method is callable, so
/// this writes a minimal literal request into one end of a duplex pair
/// and reads it back off the other end.
pub(crate) async fn test_session() -> Session {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(b"GET /dye-test HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .expect("write minimal request into duplex pair");
    drop(client); // header bytes are already buffered; no further writes needed
    session
        .read_request()
        .await
        .expect("read_request should succeed")
        .then_some(())
        .expect("client must not have closed before sending the request");
    session
}

#[tokio::test]
async fn graphql_bodyless_get_inspection_freezes_future_replay_owner() {
    use crate::plugins::http::common::graphql_request;
    let _lock = graphql_request::EVALUATION_TEST_LOCK.lock().await;
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(false);
    ctx.request_info.in_request_body_stage = true;
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    adapter.set_request_query("query=%7Bping%7D").unwrap();
    assert!(
        graphql_request::evaluate(&mut adapter, &mut PluginLog::new("GraphqlGuard"), 65536, |_| Ok(()))
            .await
            .is_ok()
    );
    assert!(!adapter.ctx().request_body.has_cached_body());
    assert_eq!(
        adapter
            .overwrite_request_body(Bytes::from_static(br#"{"query":"{pong}"}"#))
            .await,
        Err(BodyReadError::NotBuffered)
    );
    assert!(adapter.ctx().request_body.has_cached_body());
    assert_eq!(adapter.ctx().request_body.cached_len(), Ok(Some(0)));
    assert_eq!(adapter.ctx().request_body.read_all().await.unwrap(), Some(Bytes::new()));
}

#[tokio::test]
async fn response_condition_source_allocation_inventory() {
    use crate::ctx::{ClientCertInfo, NormalizedHostname};
    use crate::plugins::runtime::conditions::{extract_jwt_claim_values, extract_mtls_values, MtlsField, SanType};

    let mut session = test_session_raw(
        "GET /alpha?x=one HTTP/1.1\r\nHost: example.com\r\nX-Mode: enabled\r\nCookie: sid=abc\r\n\r\n",
    )
    .await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.trace_id.initialize(&session.req_header().headers);
    ctx.request_info.direct_peer_ip = "192.0.2.1".to_string();
    ctx.request_info.remote_ip = "198.51.100.2".to_string();
    ctx.request_info.hostname = NormalizedHostname::new("example.com");
    ctx.request_info.entry_raw_path = "/alpha".to_string();
    ctx.request_info.normalized_current_path = "/alpha".to_string();
    ctx.request_info.entry_raw_query = Some("x=one".to_string());
    ctx.set_ctx_var("demo".to_string(), "value".to_string());
    ctx.set_ctx_var("jwt_claims".to_string(), r#"{"roles":["admin","dev"]}"#.to_string());
    ctx.request_info.install_tls_info(
        None,
        None,
        Some(ClientCertInfo {
            subject: "CN=alice".to_string(),
            sans: vec![
                "alice.example.com".to_string(),
                "uri:spiffe://example.com/alice".to_string(),
            ],
            cn: Some("alice".to_string()),
            fingerprint: "ab:cd".to_string(),
        }),
        None,
        None,
    );
    // Prime the once-per-request query parse so this measures only the
    // per-chunk value extraction cost.
    ctx.cached_query();
    let adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    let cases = [
        ("traceId", KeyGet::TraceId),
        ("remoteIp", KeyGet::RemoteIp),
        ("directPeerIp", KeyGet::DirectPeerIp),
        ("host", KeyGet::Host),
        (
            "header",
            KeyGet::Header {
                name: "X-Mode".to_string(),
            },
        ),
        (
            "cookie",
            KeyGet::Cookie {
                name: "sid".to_string(),
            },
        ),
        ("query", KeyGet::Query { name: "x".to_string() }),
        ("path", KeyGet::Path),
        ("requestPath", KeyGet::RequestPath),
        ("method", KeyGet::Method),
        (
            "ctx",
            KeyGet::Ctx {
                name: "demo".to_string(),
            },
        ),
        ("remoteIpAndPath", KeyGet::RemoteIpAndPath),
    ];
    for (label, key) in cases {
        let (value, stats) =
            crate::test_allocation_counter::count_allocation_stats(|| std::hint::black_box(adapter.key_get(&key)));
        assert!(value.is_some(), "{label} fixture must be present");
        eprintln!(
            "condition source {label}: {} allocations, {} requested bytes",
            stats.calls, stats.requested_bytes
        );
    }

    let (claims, jwt_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        std::hint::black_box(extract_jwt_claim_values(&adapter, "roles"))
    });
    assert_eq!(claims, ["admin", "dev"]);
    eprintln!(
        "condition source jwtClaimMatch: {} allocations, {} requested bytes",
        jwt_stats.calls, jwt_stats.requested_bytes
    );

    let (sans, mtls_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        std::hint::black_box(extract_mtls_values(&adapter, MtlsField::San, Some(&SanType::Uri)).collect::<Vec<_>>())
    });
    assert_eq!(sans, ["spiffe://example.com/alice"]);
    eprintln!(
        "condition source mtlsMatch: {} allocations, {} requested bytes",
        mtls_stats.calls, mtls_stats.requested_bytes
    );
}

#[tokio::test]
async fn plugin_session_grpc_classification_includes_grpc_route_match() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.is_grpc_request = false;
    ctx.is_grpc_route_matched = true;
    let adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert!(PluginSession::is_grpc_request(&adapter));
    assert!(adapter.is_grpc_request());
}

#[tokio::test]
async fn template_copies_preserve_sensitive_header_and_cookie_sources() {
    use crate::plugins::http::common::outbound::request::CompiledRequest;
    use edgion_resources::resources::common::outbound_request::OutboundRequestSpec;
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    // The same typed setter used by JWE/auth plugin header producers.
    for (name, value) in [
        ("x-decrypted", "private-plaintext"),
        ("cookie", "session=private-cookie"),
    ] {
        let mut value = http::HeaderValue::from_static(value);
        value.set_sensitive(true);
        adapter.set_request_header_value(name, value).unwrap();
    }
    let spec: OutboundRequestSpec = serde_json::from_value(serde_json::json!({
        "path": {"template": "/verify"}, "method": {"template": "GET"},
        "headers": {"custom": [
            {"name": "x-copy", "template": "prefix-${header:x-decrypted}-suffix"},
            {"name": "x-session-copy", "template": "${cookie:session}"},
            {"name": "x-public", "template": "${method}"}
        ]}
    }))
    .unwrap();
    let request = CompiledRequest::from_spec(&spec)
        .unwrap()
        .build_request(&adapter, None, "http://upstream.test", None, &mut Default::default())
        .unwrap();
    for request in [&request, &request.try_clone().unwrap()] {
        assert_eq!(request.headers()["x-copy"], "prefix-private-plaintext-suffix");
        assert_eq!(request.headers()["x-session-copy"], "private-cookie");
        assert!(request.headers()["x-copy"].is_sensitive());
        assert!(request.headers()["x-session-copy"].is_sensitive());
        assert!(!request.headers()["x-public"].is_sensitive());
        assert!(!format!("{:?}", request.headers()).contains("private-plaintext"));
        assert!(!format!("{:?}", request.headers()).contains("private-cookie"));
    }
}

#[tokio::test]
async fn typed_request_header_preserves_sensitivity_and_replaces_deferred_credential() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    let name = http::HeaderName::from_static("x-private-identity");
    let mut old = http::HeaderValue::from_static("old-credential");
    old.set_sensitive(true);
    ctx.upsert_deferred_credential_mutation(name.clone(), Arc::new(old), None);
    session
        .req_header_mut()
        .append_header(name.clone(), "first-line")
        .unwrap();
    session
        .req_header_mut()
        .append_header(name.clone(), "second-line")
        .unwrap();
    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        let mut value = http::HeaderValue::from_static("current-identity");
        value.set_sensitive(true);
        adapter.set_request_header_value(name.as_str(), value).unwrap();
    }
    assert_eq!(session.req_header().headers.get_all(&name).iter().count(), 1);
    let value = &session.req_header().headers[&name];
    assert_eq!(value.as_bytes(), b"current-identity");
    assert!(value.is_sensitive());
    assert!(!format!("{value:?}").contains("current-identity"));
    let mut upstream = session.req_header().clone();
    ctx.apply_deferred_credentials(&mut upstream).unwrap();
    assert_eq!(upstream.headers[&name].as_bytes(), b"current-identity");
    assert!(upstream.headers[&name].is_sensitive());
}

#[tokio::test]
async fn typed_request_append_preserves_opaque_duplicates_and_invalidates_deferred_credentials() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    let name = http::HeaderName::from_static("x-private-identity");
    ctx.upsert_deferred_credential_mutation(name.clone(), Arc::new(http::HeaderValue::from_static("deferred")), None);
    session.req_header_mut().append_header(name.clone(), "first").unwrap();
    let mut value = http::HeaderValue::from_bytes(b"\xffidentity").unwrap();
    value.set_sensitive(true);
    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert!(adapter
            .append_request_header_value("invalid name", value.clone())
            .is_err());
    }
    let mut upstream = session.req_header().clone();
    ctx.apply_deferred_credentials(&mut upstream).unwrap();
    assert_eq!(upstream.headers[&name], "deferred");
    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter.append_request_header_value(name.as_str(), value).unwrap();
    }
    let mut upstream = session.req_header().clone();
    ctx.apply_deferred_credentials(&mut upstream).unwrap();
    let values: Vec<_> = upstream.headers.get_all(&name).iter().collect();
    assert_eq!(values.len(), 2);
    assert_eq!(values[0], "first");
    assert_eq!(values[1].as_bytes(), b"\xffidentity");
    assert!(values[1].is_sensitive());
}

#[tokio::test]
async fn response_policy_protects_bound_and_queued_headers_without_blocking_application_fields() {
    let mut session = test_session().await;
    for bound in [false, true] {
        let mut ctx = EdgionHttpContext::default();
        let mut response = ResponseHeader::build(101, None).unwrap();
        response.insert_header("content-length", "42").unwrap();
        response.insert_header("connection", "upgrade").unwrap();
        response.insert_header("upgrade", "websocket").unwrap();
        {
            let mut adapter = if bound {
                PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response)
            } else {
                PingoraSessionAdapter::new(&mut session, &mut ctx)
            };
            for name in [
                "Content-Length",
                "TRANSFER-Encoding",
                "Connection",
                "Keep-Alive",
                "Proxy-Connection",
                "TE",
                "Trailer",
                "Upgrade",
                "Proxy-Authenticate",
                "Proxy-Authorization",
                ":status",
            ] {
                assert!(adapter.set_response_header(name, "1").is_err(), "{name}");
                assert!(adapter.append_response_header(name, "1").is_err(), "{name}");
                assert!(adapter
                    .set_response_header_value(name, http::HeaderValue::from_static("1"))
                    .is_err());
                assert!(adapter
                    .append_response_header_value(name, http::HeaderValue::from_static("1"))
                    .is_err());
                assert!(adapter.remove_response_header(name).is_err());
                assert!(adapter
                    .queue_local_reply_header_op(PendingHeaderOp::Remove { name: name.into() })
                    .is_err());
            }
            for name in [
                "Content-Type",
                "Content-Encoding",
                "Cache-Control",
                "WWW-Authenticate",
                "grpc-status",
            ] {
                adapter.set_response_header(name, "application-value").unwrap();
            }
            adapter.set_response_header("Set-Cookie", "a=1").unwrap();
            adapter.append_response_header("set-cookie", "b=2").unwrap();
        }
        ctx.drain_pending_response_header_ops(&mut response);
        assert_eq!(response.headers["content-length"], "42");
        assert_eq!(response.headers["connection"], "upgrade");
        assert_eq!(response.headers["upgrade"], "websocket");
        assert_eq!(response.headers["content-encoding"], "application-value");
        assert_eq!(response.headers.get_all("set-cookie").iter().count(), 2);
    }
}

#[test]
fn direct_pending_operations_cannot_modify_framing() {
    let value = http::HeaderValue::from_static("999");
    for op in [
        PendingHeaderOp::Set {
            name: "Content-Length".into(),
            value: value.clone(),
        },
        PendingHeaderOp::Append {
            name: "Content-Length".into(),
            value: value.clone(),
        },
        PendingHeaderOp::AddIfAbsent {
            name: "Content-Length".into(),
            value: value.clone(),
        },
        PendingHeaderOp::OverwriteIfExists {
            name: "Content-Length".into(),
            value,
        },
        PendingHeaderOp::Remove {
            name: "Content-Length".into(),
        },
    ] {
        for present in [false, true] {
            let mut ctx = EdgionHttpContext::default();
            let mut response = ResponseHeader::build(200, None).unwrap();
            if present {
                response.insert_header("content-length", "2").unwrap();
            }
            ctx.queue_pending_response_header_op(op.clone());
            assert!(!ctx.has_pending_response_header_ops());
            op.clone().apply(&mut response);
            assert_eq!(
                response.headers.get("content-length").map(|v| v.to_str().unwrap()),
                present.then_some("2")
            );
        }
    }
}

#[tokio::test]
async fn typed_response_values_preserve_bytes_sensitivity_and_pending_replay() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    let mut value = http::HeaderValue::from_bytes(b"\xffprivate").unwrap();
    value.set_sensitive(true);
    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter.set_response_header_value("x-private", value.clone()).unwrap();
        adapter
            .append_response_header_value("x-private", value.clone())
            .unwrap();
        assert!(!adapter.has_response_header("x-private"));
        assert!(adapter.response_header_map().is_empty());
        assert!(adapter.response_headers_bytes().is_empty());
    }
    let mut response = ResponseHeader::build(200, None).unwrap();
    // Normal upstream response application clones the queue; local replies consume it.
    ctx.apply_pending_response_header_ops(&mut response);
    assert_eq!(response.headers.get_all("x-private").iter().count(), 2);
    for item in response.headers.get_all("x-private") {
        assert_eq!(item.as_bytes(), b"\xffprivate");
        assert!(item.is_sensitive());
    }
    let mut local_reply = ResponseHeader::build(403, None).unwrap();
    ctx.drain_pending_response_header_ops(&mut local_reply);
    assert_eq!(local_reply.headers.get_all("x-private").iter().count(), 2);
    assert!(local_reply
        .headers
        .get_all("x-private")
        .iter()
        .all(http::HeaderValue::is_sensitive));
    {
        let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response);
        assert!(adapter.has_response_header("X-Private"));
        assert_eq!(adapter.response_header_map().get_all("x-private").iter().count(), 2);
        assert!(adapter.get_response_header("x-private").is_none());
        adapter.set_response_header_value("x-private", value.clone()).unwrap();
        adapter.append_response_header_value("x-private", value).unwrap();
    }
    assert_eq!(response.headers.get_all("x-private").iter().count(), 2);
    assert!(response
        .headers
        .get_all("x-private")
        .iter()
        .all(http::HeaderValue::is_sensitive));
}

#[tokio::test]
async fn invalid_string_response_values_keep_live_error_and_queued_noop_behavior() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert!(adapter.set_response_header("x-invalid", "bad\rvalue").is_ok());
        assert!(adapter.append_response_header("x-invalid", "bad\nvalue").is_ok());
    }
    assert!(!ctx.has_pending_response_header_ops());
    let mut response = ResponseHeader::build(200, None).unwrap();
    response.insert_header("x-invalid", "original").unwrap();
    {
        let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response);
        assert!(adapter.set_response_header("x-invalid", "bad\rvalue").is_err());
        assert!(adapter.append_response_header("x-invalid", "bad\nvalue").is_err());
    }
    assert_eq!(response.headers["x-invalid"], "original");
}

#[tokio::test]
async fn explicit_local_reply_ops_bypass_bound_upstream_and_land_on_fresh_reply() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    let mut upstream = ResponseHeader::build(200, None).unwrap();
    upstream.insert_header("x-upstream", "preserved-there").unwrap();
    {
        let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut upstream);
        adapter
            .queue_local_reply_header_op(PendingHeaderOp::Set {
                name: "x-local".to_string(),
                value: http::HeaderValue::from_static("reply"),
            })
            .unwrap();
    }
    assert!(upstream.headers.get("x-local").is_none());
    assert!(build_response_local_reply(
        &mut ctx,
        403,
        Some(Bytes::from_static(b"denied")),
        false
    ));
    let (reply, _) = ctx.take_pending_response_local_reply().unwrap();
    assert_eq!(reply.headers.get("x-local").unwrap(), "reply");
    assert!(!ctx.has_pending_response_header_ops());
}

/// Like `test_session`, but keeps the client half of the duplex pair
/// alive and returns it to the caller. `write_response_header` tests
/// (Task 10) actually write bytes back over the wire; dropping the
/// client half (as `test_session` does once the request is buffered)
/// closes the reader on the other end of the duplex, so the response
/// write fails with `BrokenPipe`. The returned client is unused beyond
/// keeping the pipe open -- callers just need to hold it until the
/// session is done writing.
async fn test_session_with_client() -> (Session, tokio::io::DuplexStream) {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(b"GET /dye-test HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .expect("write minimal request into duplex pair");
    session
        .read_request()
        .await
        .expect("read_request should succeed")
        .then_some(())
        .expect("client must not have closed before sending the request");
    (session, client)
}

#[tokio::test]
async fn response_trailer_capability_tracks_actual_h1_framing() {
    let (mut session, _client) = test_session_with_client().await;
    let mut ctx = EdgionHttpContext::default();
    {
        let adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert!(
            !adapter.downstream_response_trailers_representable(),
            "an uncommitted H1 response must not claim trailer capability"
        );
    }

    let mut response = ResponseHeader::build(200, None).expect("response header");
    response
        .insert_header(http::header::TRANSFER_ENCODING, "chunked")
        .expect("valid chunked framing");
    session
        .write_response_header(Box::new(response), false)
        .await
        .expect("commit chunked H1.1 response");
    let adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert!(
        adapter.downstream_response_trailers_representable(),
        "committed chunked H1.1 framing can represent trailers"
    );
}

#[tokio::test]
async fn h1_new_response_trailer_name_requires_committed_declaration() {
    let (mut session, _client) = test_session_with_client().await;
    let mut response = ResponseHeader::build(200, None).expect("response header");
    response
        .insert_header(http::header::TRANSFER_ENCODING, "chunked")
        .expect("valid chunked framing");
    response
        .insert_header(http::header::TRAILER, "x-declared, x-second")
        .expect("valid trailer declaration");
    session
        .write_response_header(Box::new(response), false)
        .await
        .expect("commit chunked H1.1 response");
    let mut ctx = EdgionHttpContext::default();
    let adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert!(adapter.downstream_response_trailer_name_allowed(&http::HeaderName::from_static("x-declared")));
    assert!(adapter.downstream_response_trailer_name_allowed(&http::HeaderName::from_static("x-second")));
    assert!(!adapter.downstream_response_trailer_name_allowed(&http::HeaderName::from_static("x-undeclared")));
}

#[tokio::test]
async fn response_trailer_capability_rejects_h1_0() {
    let (mut session, _client) = test_session_with_client().await;
    session.req_header_mut().set_version(http::Version::HTTP_10);
    let response = ResponseHeader::build(200, None).expect("response header");
    session
        .write_response_header(Box::new(response), false)
        .await
        .expect("commit close-delimited H1.0 response");
    let mut ctx = EdgionHttpContext::default();
    let adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);

    assert!(!adapter.downstream_response_trailers_representable());
}

async fn test_session_with_body(body: &[u8]) -> Session {
    let capacity = body.len().saturating_add(1024).max(4096);
    let (mut client, server) = tokio::io::duplex(capacity);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    let header = format!(
        "POST /body-test HTTP/1.1\r\nHost: test\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    client
        .write_all(header.as_bytes())
        .await
        .expect("write request headers into duplex pair");
    client
        .write_all(body)
        .await
        .expect("write request body into duplex pair");
    drop(client);
    session
        .read_request()
        .await
        .expect("read_request should succeed")
        .then_some(())
        .expect("client must not close before sending the request");
    session
}

#[tokio::test]
async fn waf_prefix_reuses_storage_and_preserves_the_unread_remainder() {
    let max_prefix = edgion_waf::seclang::MAX_REQUEST_BODY_PREFIX_BYTES;
    for (len, limit) in [(3, 4), (4, 4), (5, 4), (100_000, 4), (max_prefix + 1, max_prefix)] {
        let original: Vec<_> = (0..len).map(|index| (index % 251) as u8).collect();
        let mut session = test_session_with_body(&original).await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = true;
        // Prefix capture stays in memory even when the complete-body
        // policy would spill at a lower threshold.
        ctx.request_body_config.memory_buffer_bytes = 2;
        {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            let (prefix, complete) = adapter.read_request_body_prefix(limit).await.unwrap();
            assert_eq!(prefix.as_ref(), &original[..len.min(limit)]);
            assert_eq!(complete, len <= limit);
            assert!(!adapter.ctx.request_body.has_cached_body());
            assert!(matches!(
                adapter.ensure_request_body(None).await,
                Err(BodyReadError::NotBuffered)
            ));
            assert!(matches!(
                adapter.overwrite_request_body(Bytes::new()).await,
                Err(BodyReadError::NotBuffered)
            ));
            assert!(!adapter.install_request_body_stream_handler(Box::new(NoopStreamHandler)));
        }
        session.begin_request_body_replay().await.unwrap();
        let mut forwarded = Vec::new();
        while !session.is_body_done() {
            if let Some(chunk) = session.read_body_or_idle(false).await.unwrap() {
                forwarded.extend_from_slice(&chunk);
            }
        }
        assert_eq!(forwarded, original);
        assert_eq!(session.body_bytes_read(), len);
    }
}

#[tokio::test]
async fn waf_prefix_reuses_an_existing_complete_body_and_freezes_mutation() {
    let mut session = test_session_with_body(b"original-body").await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(true);
    ctx.request_info.in_request_body_stage = true;
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    adapter.ensure_request_body(None).await.unwrap();
    adapter
        .overwrite_request_body(Bytes::from_static(b"edited-body"))
        .await
        .unwrap();
    let (prefix, complete) = adapter.read_request_body_prefix(6).await.unwrap();
    assert_eq!(prefix.as_ref(), b"edited");
    assert!(!complete);
    assert!(!adapter.inner.request_body_prefix_active());
    assert_eq!(adapter.read_request_body(None).await.unwrap().as_ref(), b"edited-body");
    assert!(adapter
        .ctx
        .request_body
        .overwrite(Bytes::from_static(b"bypass"))
        .is_err());
    assert!(adapter
        .request_body_append(Bytes::from_static(b"bypass"))
        .await
        .is_err());
}

#[tokio::test]
async fn waf_prefix_rejects_a_declared_body_above_the_physical_cap_before_reading() {
    let mut session = test_session_with_body(b"12345").await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(true);
    ctx.request_info.in_request_body_stage = true;
    ctx.request_body_config.max_body_size_bytes = 4;
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert!(matches!(
        adapter.read_request_body_prefix(64).await,
        Err(BodyReadError::BufferLimitExceeded { limit: 4 })
    ));
    assert_eq!(adapter.inner.body_bytes_read(), 0);
}

fn compile_plan(yaml: &str, type_prefix: &'static str) -> Arc<CompiledDyePlan> {
    let suite: DyeSuite = serde_yaml::from_str(yaml).unwrap();
    CompiledDyePlan::compile(&suite, type_prefix).unwrap()
}

fn pending_ops(ctx: &EdgionHttpContext) -> &[PendingHeaderOp] {
    ctx.pending_response_header_ops()
}

/// `PendingHeaderOp` has no `PartialEq` (nothing else in the codebase
/// needed one), so assert on the single queued op's shape by matching
/// instead of deriving one just for this test module.
fn assert_single_append(ctx: &EdgionHttpContext, expect_name: &str, expect_value: &str) {
    match pending_ops(ctx) {
        [PendingHeaderOp::Append { name, value }] => {
            assert_eq!(name, expect_name);
            assert_eq!(value, expect_value);
        }
        other => panic!("expected exactly one Append op, got {other:?}"),
    }
}

fn assert_single_set(ctx: &EdgionHttpContext, expect_name: &str, expect_value: &str) {
    match pending_ops(ctx) {
        [PendingHeaderOp::Set { name, value }] => {
            assert_eq!(name, expect_name);
            assert_eq!(value, expect_value);
        }
        other => panic!("expected exactly one Set op, got {other:?}"),
    }
}

#[tokio::test]
async fn record_outcome_failure_appends_prefixed_value_to_queue() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.install_current_dye_plan(Some(compile_plan(
        "response:\n  - name: X-O\n    on: [fail-open]\n",
        "rate_limit_redis",
    )));

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    adapter.record_outcome(DyeOutcome::FailOpen, "redis-unreachable");

    assert_single_append(&ctx, "X-O", "rate_limit_redis:fail-open:redis-unreachable");
}

#[tokio::test]
async fn set_remote_ip_canonicalizes_and_invalidates_cached_geo() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.remote_ip = "192.0.2.1".to_string();
    let _ = ctx.remote_geo();
    assert!(ctx.request_info.resolved_geo().0.is_some());

    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter.set_remote_ip("::ffff:203.0.113.9").unwrap();
    }

    assert_eq!(ctx.request_info.remote_ip, "203.0.113.9");
    assert!(ctx.request_info.resolved_geo().0.is_none());
}

#[tokio::test]
async fn record_outcome_success_sets_recorded_value_verbatim() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    // Omitted `on` defaults to [Success]; omitted `value` renders the
    // recorded value (no operator template, no provenance prefix).
    ctx.install_current_dye_plan(Some(compile_plan("response:\n  - name: X-O\n", "canary")));

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    adapter.record_outcome(DyeOutcome::Success, "app-v1");

    assert_single_set(&ctx, "X-O", "app-v1");
}

struct NoopStreamHandler;

#[async_trait::async_trait]
impl crate::plugins::runtime::traits::RequestBodyStreamHandler for NoopStreamHandler {
    async fn on_chunk(
        &mut self,
        _chunk: &mut Option<Bytes>,
        _event: RequestBodyEvent,
    ) -> crate::plugins::runtime::traits::ChunkVerdict {
        crate::plugins::runtime::traits::ChunkVerdict::Continue
    }
    fn take_log(&mut self) -> Option<PluginLog> {
        None
    }
    fn dye_plan(&self) -> Option<Arc<CompiledDyePlan>> {
        None
    }
}

struct NoopObserver;

struct OversizedObserver;

struct PanickingFactoryCapture;

impl Drop for PanickingFactoryCapture {
    fn drop(&mut self) {
        panic!("captured factory detail must stay redacted");
    }
}

impl crate::request_body::observer::sealed::Sealed for NoopObserver {}

impl crate::request_body::observer::RequestBodyObserver for NoopObserver {
    fn retained_bytes(&self) -> usize {
        1
    }

    fn observe(
        &mut self,
        _bytes: &[u8],
        _event: RequestBodyEvent,
        _budget: &mut crate::request_body::observer::ObserverBudget,
    ) -> Result<crate::request_body::observer::ObserverControl, crate::request_body::observer::ObserverError> {
        Ok(crate::request_body::observer::ObserverControl::Continue)
    }
}

impl crate::request_body::observer::sealed::Sealed for OversizedObserver {}

impl crate::request_body::observer::RequestBodyObserver for OversizedObserver {
    fn retained_bytes(&self) -> usize {
        crate::request_body::observer::MAX_OBSERVER_RETAINED_BYTES + 1
    }

    fn observe(
        &mut self,
        _bytes: &[u8],
        _event: RequestBodyEvent,
        _budget: &mut crate::request_body::observer::ObserverBudget,
    ) -> Result<crate::request_body::observer::ObserverControl, crate::request_body::observer::ObserverError> {
        Ok(crate::request_body::observer::ObserverControl::Continue)
    }
}

/// A real H1 downstream can answer the transport facts, so the streamed
/// handler seam installs into the ctx vector and sets the monotonic latch.
#[tokio::test]
async fn h1_session_installs_request_body_stream_handler() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.in_request_body_stage = true;
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert!(adapter.request_headers_end_stream().is_some());
    assert!(adapter.install_request_body_stream_handler(Box::new(NoopStreamHandler)));
    assert_eq!(ctx.request_body_stream_handler_count(), 1);
    assert!(
        ctx.request_body_stream_active(),
        "install must set the monotonic retry/disposition latch"
    );
    let handlers = ctx.take_request_body_stream_handlers();
    assert_eq!(handlers.len(), 1);
    assert_eq!(ctx.request_body_stream_handler_count(), 0);
    assert!(
        ctx.request_body_stream_active(),
        "losing handlers across a cancelled callback must not re-arm replay"
    );
    drop(handlers);
}

#[tokio::test]
async fn streamed_handler_install_refuses_after_body_policy_freeze() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.in_request_body_stage = false;
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);

    assert!(!adapter.install_request_body_stream_handler(Box::new(NoopStreamHandler)));
    assert_eq!(ctx.request_body_stream_handler_count(), 0);
    assert!(!ctx.request_body_stream_active());
}

#[tokio::test]
async fn mirror_body_tap_install_refuses_after_body_policy_freeze() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.in_request_body_stage = false;
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);

    assert!(!adapter.set_mirror_state(MirrorState::Abandoned));
    assert!(!ctx.has_mirror_state());
}

#[tokio::test]
async fn backend_mirror_permission_is_first_attempt_only_and_adapter_local() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    // A session outside Pingora's attempt loop cannot grant permission.
    assert!(
        !PingoraSessionAdapter::for_backend_request(&mut session, &mut ctx).set_mirror_state(MirrorState::Abandoned)
    );

    // Pingora owns the attempt ID constructor. Exercise the same decision
    // with all boundary values; real first-attempt wiring is covered by
    // the RequestMirrorReferenceGrant integration scenario.
    for (attempt, allowed) in [(None, false), (Some(0), false), (Some(1), true), (Some(2), false)] {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter.backend_mirror_allowed = backend_mirror_allowed_for_attempt(attempt);
        assert_eq!(adapter.set_mirror_state(MirrorState::Abandoned), allowed);
        assert_eq!(adapter.ctx.has_mirror_state(), allowed);
        adapter.ctx.retire_request_mirror();
    }

    let mut ordinary = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert!(!ordinary.set_mirror_state(MirrorState::Abandoned));
}

#[tokio::test]
async fn backend_mirror_permission_keeps_other_body_capabilities_frozen() {
    let mut session = test_session_with_body(b"body").await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(true);
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    adapter.backend_mirror_allowed = backend_mirror_allowed_for_attempt(Some(1));

    assert!(matches!(
        adapter.ensure_request_body(None).await,
        Err(BodyReadError::NotBuffered)
    ));
    assert!(matches!(
        adapter.replace_request_body_spans(0, &[]).await,
        Err(BodyReadError::NotBuffered)
    ));
    assert!(!adapter.install_request_body_stream_handler(Box::new(NoopStreamHandler)));
    assert!(!adapter.waf().enable_request_body_streaming(&Arc::new(()), 1024));
    adapter.replace_plugin_entry_identity(crate::ctx::CurrentPluginEntryIdentity::new("AiGuard", Some("entry")));
    assert!(!adapter.install_request_body_observer(
        Box::new(|| panic!("a frozen observer factory must not run")),
        crate::request_body::observer::ObserverLimits::default(),
    ));
    assert!(!adapter.ctx.request_body.has_cached_body());
    assert!(!adapter.ctx.has_request_body_plugins());

    let (body_tx, _body_rx) = tokio::sync::mpsc::channel(1);
    let (cancellation, cancel_rx) = crate::ctx::MirrorCancellation::new();
    assert!(adapter.set_mirror_state(MirrorState::Streaming {
        body_tx,
        writer_handle: tokio::spawn(async {}),
        cancellation,
        channel_full_timeout_ms: 0,
        body_budget: crate::plugins::http::request_mirror::MirrorBodyBudget::for_test(1024),
    }));
    assert!(!adapter.ctx.request_info.in_request_body_stage);
    assert!(
        !adapter.ctx.request_body_stream_active(),
        "a mirror must not disable replay"
    );
    adapter.ctx.retire_request_mirror();
    assert!(cancel_rx.await.is_ok());
}

#[tokio::test]
async fn observer_install_requires_request_stage_and_runtime_identity() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.in_request_body_stage = true;
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    let make_factory = || {
        Box::new(|| Ok(Box::new(NoopObserver) as Box<dyn crate::request_body::observer::RequestBodyObserver>))
            as crate::request_body::observer::RequestBodyObserverFactory
    };

    assert!(!adapter
        .install_request_body_observer(make_factory(), crate::request_body::observer::ObserverLimits::default(),));
    assert!(!adapter.ctx.has_request_body_observers());

    let previous =
        adapter.replace_plugin_entry_identity(crate::ctx::CurrentPluginEntryIdentity::new("AiGuard", Some("entry")));
    assert!(!adapter.install_request_body_observer(
        Box::new(|| Err(crate::request_body::observer::ObserverInitError)),
        crate::request_body::observer::ObserverLimits::default(),
    ));
    assert!(!adapter.ctx.has_request_body_observers());
    assert!(!adapter.install_request_body_observer(
        Box::new(|| panic!("static factory panic")),
        crate::request_body::observer::ObserverLimits::default(),
    ));
    assert!(!adapter.ctx.has_request_body_observers());
    assert!(!adapter.install_request_body_observer(
        Box::new(|| Ok(Box::new(OversizedObserver))),
        crate::request_body::observer::ObserverLimits::default(),
    ));
    assert!(!adapter.ctx.has_request_body_observers());
    assert!(adapter
        .install_request_body_observer(make_factory(), crate::request_body::observer::ObserverLimits::default(),));
    adapter.restore_plugin_entry_identity(previous);
    assert!(adapter.ctx.has_request_body_observers());
    assert!(!adapter.ctx.request_body_stream_active());
}

#[tokio::test]
async fn observer_registration_refuses_unsupported_stage_and_h1_transports() {
    #[derive(Clone, Copy)]
    enum Refusal {
        Stage,
        Connect,
        Upgrade,
        Http10,
    }

    async fn refused(case: Refusal) {
        let mut session = test_session().await;
        match case {
            Refusal::Stage => {}
            Refusal::Connect => session.req_header_mut().set_method(http::Method::CONNECT),
            Refusal::Upgrade => {
                session.req_header_mut().insert_header("connection", "upgrade").unwrap();
                session.req_header_mut().insert_header("upgrade", "websocket").unwrap();
            }
            Refusal::Http10 => session.req_header_mut().set_version(http::Version::HTTP_10),
        }
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.in_request_body_stage = !matches!(case, Refusal::Stage);
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        let previous = adapter
            .replace_plugin_entry_identity(crate::ctx::CurrentPluginEntryIdentity::new("AiGuard", Some("entry")));
        let captured = PanickingFactoryCapture;
        let factory = Box::new(move || {
            let _captured = &captured;
            Ok(Box::new(NoopObserver) as Box<dyn crate::request_body::observer::RequestBodyObserver>)
        });
        assert!(
            !adapter.install_request_body_observer(factory, crate::request_body::observer::ObserverLimits::default(),)
        );
        adapter.restore_plugin_entry_identity(previous);
        assert!(!adapter.ctx.has_request_body_observers());
        assert!(!adapter.ctx.request_body_stream_active());
    }

    for case in [Refusal::Stage, Refusal::Connect, Refusal::Upgrade, Refusal::Http10] {
        refused(case).await;
    }
}

#[tokio::test]
async fn plugin_entry_identity_nested_restore_is_exact() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);

    let before_outer = adapter.replace_plugin_entry_identity(crate::ctx::CurrentPluginEntryIdentity::new(
        "ExtensionRef",
        Some("outer"),
    ));
    assert!(before_outer.is_none());
    let before_inner =
        adapter.replace_plugin_entry_identity(crate::ctx::CurrentPluginEntryIdentity::new("AiGuard", Some("inner")));
    assert_eq!(
        before_inner.as_ref().map(|identity| identity.name()),
        Some("ExtensionRef")
    );
    adapter.restore_plugin_entry_identity(before_inner);
    assert_eq!(
        adapter
            .ctx
            .current_plugin_entry_identity()
            .as_ref()
            .map(|identity| identity.name()),
        Some("ExtensionRef")
    );
    assert_eq!(
        adapter
            .ctx
            .current_plugin_entry_identity()
            .as_ref()
            .and_then(|identity| identity.alias()),
        Some("outer")
    );
    adapter.restore_plugin_entry_identity(before_outer);
    assert!(adapter.ctx.current_plugin_entry_identity().is_none());
}

#[tokio::test]
async fn record_outcome_no_plan_is_noop() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    // No plan installed at all.
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    adapter.record_outcome(DyeOutcome::FailOpen, "cause");
    assert!(pending_ops(&ctx).is_empty());
}

#[tokio::test]
async fn record_outcome_no_matching_rule_is_noop() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    // Plan installed, but the only rule declares `on: [success]` while
    // the recorded outcome is a failure -- no rule matches.
    ctx.install_current_dye_plan(Some(compile_plan("response:\n  - name: X-O\n", "canary")));
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    adapter.record_outcome(DyeOutcome::FailOpen, "cause");
    assert!(pending_ops(&ctx).is_empty());
}

#[tokio::test]
async fn record_outcome_template_value_renders_and_sanitizes() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.install_current_dye_plan(Some(compile_plan(
        "response:\n  - name: X-O\n    on: [fail-open]\n    value: 'v ${ctx:id}'\n",
        "canary",
    )));

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    // Control bytes on a rendered value can only arrive through a
    // non-header-validated source (an `http::HeaderValue` rejects raw
    // CR/LF outright, so a wire-level request header can never carry
    // them) -- a ctx var set by an earlier plugin (webhook response, JWT
    // claim, ...) is the realistic vector `sanitize_field_value` guards.
    adapter.set_ctx_var("id", "abc\r\ninjected").unwrap();
    adapter.record_outcome(DyeOutcome::FailOpen, "unused");

    assert_single_append(&ctx, "X-O", "v abcinjected");
}

#[tokio::test]
async fn record_outcome_bound_response_header_writes_directly() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.install_current_dye_plan(Some(compile_plan(
        "response:\n  - name: X-O\n    on: [fail-open]\n",
        "canary",
    )));
    let mut response_header = ResponseHeader::build(200, None).unwrap();

    let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
    adapter.record_outcome(DyeOutcome::FailOpen, "cause");

    // D-1 regression pin: a response-stage record must land directly in
    // the bound header via the adapter's existing dual-mode
    // set_response_header/append_response_header, never bypass it by
    // pushing straight onto `pending_response_header_ops`.
    assert_eq!(
        response_header.headers.get("X-O").unwrap().to_str().unwrap(),
        "canary:fail-open:cause"
    );
    assert!(pending_ops(&ctx).is_empty());
}

/// Task 10 (write_response_header drain amendment): a plugin that
/// self-writes its response via `write_response_header` (the
/// ErrTerminateRequest fail-close path -- rate_limit_redis,
/// forward_auth, ldap_auth) must still carry any queued dye/header ops,
/// same as the runtime's own local-reply / gRPC error builders. But
/// unlike those builders, the caller here writes the body SEPARATELY
/// afterwards, so a queued framing op (Set content-length here) must
/// NOT win over the caller's own framing -- the caller's
/// content-length/transfer-encoding must be restored after the drain.
#[tokio::test]
async fn self_written_response_drains_queue_with_framing_protected() {
    let (mut session, _client) = test_session_with_client().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.queue_pending_response_header_op(PendingHeaderOp::Append {
        name: "X-O".to_string(),
        value: http::HeaderValue::from_static("v1"),
    });
    // Hostile framing op: without protection this would clobber the
    // caller's real content-length and desync the wire from the body
    // the caller is about to write in a separate call.
    ctx.queue_pending_response_header_op(PendingHeaderOp::Set {
        name: "content-length".to_string(),
        value: http::HeaderValue::from_static("999"),
    });

    let mut resp = ResponseHeader::build(200, None).unwrap();
    resp.insert_header("content-length", "5").unwrap();

    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter
            .write_response_header(Box::new(resp), false)
            .await
            .expect("write_response_header should succeed");
    }

    let written = session
        .response_written()
        .expect("header must have been written to the session");
    assert_eq!(written.headers.get("X-O").unwrap().to_str().unwrap(), "v1");
    assert_eq!(
        written.headers.get("content-length").unwrap().to_str().unwrap(),
        "5",
        "caller's original content-length must survive a queued hostile Set"
    );
    assert!(
        written.headers.get("transfer-encoding").is_none(),
        "no transfer-encoding should be introduced by the drain"
    );
    assert!(pending_ops(&ctx).is_empty());
}

#[tokio::test]
async fn direct_writer_cannot_claim_an_informational_response() {
    let (mut session, _client) = test_session_with_client().await;
    let mut ctx = EdgionHttpContext::default();
    let resp = ResponseHeader::build(103, None).expect("103 is a valid informational header");

    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter
            .write_response_header(Box::new(resp), true)
            .await
            .expect_err("an informational status cannot be a terminal local reply");
    }

    assert!(session.response_written().is_none());
    assert!(!ctx.response_write_started());
    assert_eq!(ctx.final_downstream_status(), None);
}

#[tokio::test]
async fn direct_writer_reports_a_duplicate_claim_without_writing() {
    let (mut session, _client) = test_session_with_client().await;
    let mut ctx = EdgionHttpContext::default();
    assert!(ctx.claim_final_response(204));
    let resp = ResponseHeader::build(403, None).unwrap();

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    adapter
        .write_response_header(Box::new(resp), true)
        .await
        .expect_err("a second local reply must not report success");

    assert!(session.response_written().is_none());
    assert_eq!(ctx.final_downstream_status(), Some(204));
}

#[tokio::test]
async fn grpc_writer_rejects_an_informational_local_reply() {
    let (mut session, _client) = test_session_with_client().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.is_grpc_request = true;
    let resp = ResponseHeader::build(101, None).unwrap();

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    adapter
        .write_response_header(Box::new(resp), true)
        .await
        .expect_err("an informational reply must not become gRPC trailers");

    assert!(session.response_written().is_none());
    assert_eq!(ctx.final_downstream_status(), None);
}

#[tokio::test]
async fn response_status_mutation_rejects_informational_status() {
    let (mut session, _client) = test_session_with_client().await;
    let mut ctx = EdgionHttpContext::default();
    let mut response = ResponseHeader::build(200, None).unwrap();

    {
        let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response);
        assert!(adapter.set_response_status(103).is_err());
    }

    assert_eq!(response.status.as_u16(), 200);
}

#[tokio::test]
async fn actual_switching_protocols_write_is_terminal_at_finalization() {
    let (mut session, _client) = test_session_with_client().await;
    let mut ctx = EdgionHttpContext::default();
    let response = ResponseHeader::build(101, None).unwrap();

    session
        .write_response_header(Box::new(response), true)
        .await
        .expect("101 response header should reach the transport");
    let written_status = session.response_written().map(|header| header.status.as_u16());
    ctx.finalize_downstream_status(written_status);

    assert_eq!(ctx.request_info.status(), 101);
}

/// Task 10 regression pin: the gRPC reroute inside `write_response_header`
/// (status != 200 on a gRPC route) already drained queued ops via
/// `send_grpc_trailers_only` before this task -- confirm that path is
/// untouched by the plain-HTTP-arm framing-protection amendment.
#[tokio::test]
async fn grpc_reroute_still_drains_via_trailers_path() {
    let (mut session, _client) = test_session_with_client().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.is_grpc_request = true;
    ctx.queue_pending_response_header_op(PendingHeaderOp::Append {
        name: "X-O".to_string(),
        value: http::HeaderValue::from_static("v1"),
    });

    let resp = ResponseHeader::build(404, None).unwrap();

    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter
            .write_response_header(Box::new(resp), false)
            .await
            .expect("write_response_header should succeed");
    }

    let written = session
        .response_written()
        .expect("trailers-only header must have been written");
    assert_eq!(written.headers.get("X-O").unwrap().to_str().unwrap(), "v1");
    assert!(written.headers.get("grpc-status").is_some());
    assert_eq!(ctx.final_downstream_status(), Some(200));
    assert!(ctx.request_info.grpc_status().is_some());
    assert!(pending_ops(&ctx).is_empty());
}

/// Pins the trait accessor a real adapter exposes over the ctx field,
/// using the same `test_session()` construction the rest of this module
/// already relies on for adapter-level tests.
#[tokio::test]
async fn declares_body_accessor_forwards_ctx_snapshot() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();

    ctx.request_info.body_declared = Some(true);
    let adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert_eq!(adapter.declares_body(), Some(true));
}

#[tokio::test]
async fn opaque_body_metadata_marks_transport_proven_empty_without_capture() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(false);

    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert_eq!(adapter.request_body_available_len(true), Ok(0));
        assert_eq!(adapter.request_body_available_len(false), Ok(0));
        let mut reader = adapter
            .request_body_snapshot_reader(false)
            .await
            .expect("known-empty snapshot reader");
        assert_eq!(reader.len(), 0);
        assert_eq!(reader.next_chunk().await, Ok(None));
    }

    assert!(!ctx.request_body.has_cached_body());
    assert_eq!(
        ctx.request_body
            .text_operation(crate::request_body::BodyTextOperation::IsEmpty, 0)
            .await
            .unwrap()
            .value,
        crate::request_body::BodyTextResult::Bool(true)
    );
}

#[tokio::test]
async fn runtime_disabled_capture_reports_unimplemented() {
    let mut session = test_session_with_body(b"disabled").await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(true);
    ctx.request_info.in_request_body_stage = true;
    ctx.request_body_config.enabled = false;

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert_eq!(adapter.read_request_body(None).await, Err(BodyReadError::Unimplemented));
    assert!(!ctx.request_body.has_cached_body());
}

/// Regression pin for Finding 1: `read_request_body` must derive its
/// stage signal from `ctx.request_info.in_request_body_stage`, not from
/// whether a response header happens to be bound. This is the exact
/// shape of the original bug -- `PingoraSessionAdapter::new` (no bound
/// response header) constructed at the upstream response *body* stage
/// (`plugin_runtime.rs`'s `run_upstream_response_body_plugins`), which
/// runs strictly after the request body has been proxied upstream.
/// Simulates that stage by clearing the marker directly, the same way
/// `request_filter`'s single exit-point write does in production, then
/// drives the real trait method (not the bare `request_admission`
/// predicate) so a future regression that re-derives the stage signal
/// from `response_header.is_some()` fails this test.
#[tokio::test]
async fn replace_request_body_spans_refuses_once_past_request_stage() {
    // The two sibling mutation methods omit this check, so it is asserted
    // explicitly here rather than assumed to be inherited.
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(true);
    ctx.request_info.in_request_body_stage = false;

    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        let result = adapter
            .replace_request_body_spans(0, &[(0..1, bytes::Bytes::from_static(b"x"))])
            .await;
        assert!(matches!(result, Err(BodyReadError::NotBuffered)));
    }
}

#[tokio::test]
async fn read_request_body_refuses_once_past_request_stage() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(true);
    ctx.request_info.in_request_body_stage = false;

    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        let result = adapter.read_request_body(None).await;
        assert!(matches!(result, Err(BodyReadError::NotBuffered)));
    }
    assert!(ctx.diagnostics.contains_error(EdgionErrorCode::BodyRefusedStage));
}

/// A truthfully bodyless request needs no capture and remains readable as
/// the canonical empty body after the request stage.
#[tokio::test]
async fn read_request_body_bodyless_past_request_stage_is_empty() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(false);
    ctx.request_info.in_request_body_stage = false;

    let (body, digest) = {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        let body = adapter.read_request_body(None).await.expect("bodyless late read");
        let digest = adapter.request_body_sha256().await.expect("bodyless digest");
        (body, digest)
    };

    assert!(body.is_empty());
    assert_eq!(digest, crate::request_body::EMPTY_SHA256);
    assert!(!ctx.request_body.has_cached_body());
    assert!(!ctx.diagnostics.contains_error(EdgionErrorCode::BodyRefusedStage));
}

#[tokio::test]
async fn bodyless_request_body_mutations_install_replay_owner() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(false);
    ctx.request_info.in_request_body_stage = true;

    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter.request_body_append(Bytes::from_static(b"abc")).await.unwrap();
        adapter.request_body_prepend(Bytes::from_static(b"[")).await.unwrap();
        adapter
            .request_body_replace_literal(Bytes::from_static(b"b"), Bytes::from_static(b"B"), 64)
            .await
            .unwrap();
        adapter.request_body_remove(0..1).await.unwrap();
        adapter.request_body_clear().await.unwrap();
        adapter.request_body_append(Bytes::from_static(b"x")).await.unwrap();
    }

    assert_eq!(
        ctx.request_body.read_all().await.unwrap().unwrap(),
        Bytes::from_static(b"x")
    );
    assert_eq!(ctx.request_body.modified_len_for_framing().unwrap(), Some(1));
}

/// ForwardAuth's body accessor observes the latest committed generation, in
/// memory and after spill, rather than keeping the entry payload for auth.
#[tokio::test]
async fn auth_body_read_observes_committed_rewrite() {
    for memory_limit in [64, 1] {
        let mut session = test_session_with_body(b"entry body").await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = true;
        ctx.request_body_config.memory_buffer_bytes = memory_limit;
        ctx.request_body_config.max_body_size_bytes = 64;
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert_eq!(
            adapter.read_request_body(None).await.unwrap(),
            Bytes::from_static(b"entry body")
        );
        adapter
            .request_body_replace_literal(Bytes::from_static(b"entry"), Bytes::from_static(b"rewritten"), 64)
            .await
            .unwrap();
        assert_eq!(
            adapter.read_request_body(None).await.unwrap(),
            Bytes::from_static(b"rewritten body")
        );
    }
}

/// A late consumer may reuse an existing request-scoped body cache, but
/// may not initiate capture after the request-body service window.
#[tokio::test]
async fn read_request_body_reuses_existing_cache_past_request_stage() {
    let mut session = test_session_with_body(b"buffered-before-upstream").await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(true);
    ctx.request_info.in_request_body_stage = true;

    let first = {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter.read_request_body(None).await.expect("request-stage capture")
    };
    ctx.request_info.in_request_body_stage = false;
    let second = {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter.read_request_body(None).await.expect("late cached read")
    };

    assert_eq!(first, Bytes::from_static(b"buffered-before-upstream"));
    assert_eq!(first.as_ptr(), second.as_ptr());
    assert!(!ctx.diagnostics.contains_error(EdgionErrorCode::BodyRefusedStage));
}

/// Late stages may read an existing cache, but must never acquire mutable
/// replay ownership even when that cache is still present.
#[tokio::test]
async fn overwrite_request_body_refuses_existing_cache_past_request_stage() {
    let mut session = test_session_with_body(b"immutable-after-upstream").await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(true);
    ctx.request_info.in_request_body_stage = true;

    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter.read_request_body(None).await.expect("request-stage capture");
    }
    ctx.request_info.in_request_body_stage = false;
    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        let result = adapter
            .overwrite_request_body(Bytes::from_static(b"late mutation"))
            .await;
        assert!(matches!(result, Err(BodyReadError::NotBuffered)));
    }

    assert_eq!(
        ctx.request_body.read_all().await.unwrap().unwrap(),
        Bytes::from_static(b"immutable-after-upstream")
    );
    assert!(ctx.diagnostics.contains_error(EdgionErrorCode::BodyRefusedStage));
}

/// BackendRef request plugins run after request-scoped body policy is
/// frozen. They may not initiate capture, even though the downstream pump
/// has not started yet.
#[tokio::test]
async fn read_request_body_refuses_during_backend_ref_plugin_stage() {
    let mut session = test_session_with_body(b"backend-body").await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(true);
    ctx.request_info.in_request_body_stage = false;

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert_eq!(adapter.read_request_body(None).await, Err(BodyReadError::NotBuffered));
    assert!(ctx.diagnostics.contains_error(EdgionErrorCode::BodyRefusedStage));
}

#[tokio::test]
async fn read_request_body_global_cap_is_hard_failure() {
    let mut session = test_session_with_body(b"12345678").await;
    assert!(session.get_keepalive().is_some());
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(true);
    ctx.request_info.in_request_body_stage = true;
    ctx.request_body_config.max_body_size_bytes = 4;

    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert_eq!(
        adapter.read_request_body(None).await,
        Err(BodyReadError::BufferLimitExceeded { limit: 4 })
    );
    assert!(
        session.get_keepalive().is_none(),
        "an unread over-limit HTTP/1 body must make the connection non-reusable"
    );
}

#[tokio::test]
async fn chunked_physical_cap_rejects_without_waiting_for_eof() {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(
            b"POST /body-test HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n\
                  5\r\n12345\r\n",
        )
        .await
        .expect("write an over-limit chunk without the terminating chunk");
    session.read_request().await.expect("read chunked request headers");
    assert!(session.get_keepalive().is_some());

    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(true);
    ctx.request_info.in_request_body_stage = true;
    ctx.request_body_config.max_body_size_bytes = 4;
    let result = tokio::time::timeout(std::time::Duration::from_millis(100), async {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter.read_request_body(None).await
    })
    .await
    .expect("over-limit detection must not wait for request-body EOF");

    assert_eq!(result, Err(BodyReadError::BufferLimitExceeded { limit: 4 }));
    assert!(
        session.get_keepalive().is_none(),
        "the partially unread HTTP/1 request must disable connection reuse"
    );
}

#[tokio::test]
async fn ensure_request_body_hashes_disk_without_materializing() {
    let body = vec![b'x'; DEFAULT_MEMORY_BUFFER_BYTES + 1];
    let expected: [u8; 32] = sha2::Sha256::digest(&body).into();
    let mut session = test_session_with_body(&body).await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(true);
    ctx.request_info.in_request_body_stage = true;

    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert_eq!(
            adapter
                .ensure_request_body(None)
                .await
                .expect("body capture must complete"),
            body.len() as u64
        );
        assert_eq!(
            adapter.request_body_sha256().await.expect("disk-backed body must hash"),
            expected
        );
        assert_eq!(
            adapter
                .request_body_sha256()
                .await
                .expect("the cached disk-backed digest must remain available"),
            expected
        );
    }

    assert_eq!(
        ctx.request_body.cached_body_is_materialized(),
        Some(false),
        "streaming hash must not fill read_all cache"
    );
}

#[tokio::test]
async fn request_body_snapshot_reader_streams_disk_without_materializing() {
    let body = vec![b's'; DEFAULT_MEMORY_BUFFER_BYTES + 17];
    let mut session = test_session_with_body(&body).await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(true);
    ctx.request_info.in_request_body_stage = true;

    let mut reader = {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter
            .ensure_request_body(None)
            .await
            .expect("body capture must complete");
        adapter
            .request_body_snapshot_reader(false)
            .await
            .expect("captured snapshot reader")
    };
    assert_eq!(reader.len(), body.len() as u64);

    let mut streamed = bytes::BytesMut::new();
    while let Some(chunk) = reader.next_chunk().await.expect("snapshot chunk") {
        assert!(chunk.len() <= 64 * 1024);
        streamed.extend_from_slice(&chunk);
    }
    assert_eq!(streamed.as_ref(), body.as_slice());
    assert_eq!(ctx.request_body.cached_body_is_materialized(), Some(false));
}

#[tokio::test]
async fn request_body_buffer_gate_preserves_disk_spill_without_materializing() {
    let body = vec![b'x'; DEFAULT_MEMORY_BUFFER_BYTES + 1];
    let mut session = test_session_with_body(&body).await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(true);
    ctx.request_info.in_request_body_stage = true;

    {
        let gate = BodyGatedRequestFilter::new(RequestBodyBufferPlugin::create(), BodyRequirement::fail_close());
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        let mut log = PluginLog::new("RequestBodyBuffer");
        assert_eq!(
            gate.run_request(&mut adapter, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }

    assert_eq!(
        ctx.request_body.cached_body_is_materialized(),
        Some(false),
        "the no-op prebuffer plugin must leave a disk-backed body on disk"
    );

    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert_eq!(
            adapter
                .read_request_body(None)
                .await
                .expect("a later byte consumer must still read the captured body"),
            Bytes::from(body)
        );
    }
    assert_eq!(
        ctx.request_body.cached_body_is_materialized(),
        Some(true),
        "the spill should materialize only when a later consumer requests all bytes"
    );
}

#[tokio::test]
async fn read_request_body_answers_expect_continue_before_drain() {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(b"POST /body-test HTTP/1.1\r\nHost: test\r\nContent-Length: 4\r\nExpect: 100-continue\r\n\r\n")
        .await
        .expect("write expect request headers");
    session
        .read_request()
        .await
        .expect("read_request should succeed")
        .then_some(())
        .expect("client must not close before sending request headers");

    let client_task = tokio::spawn(async move {
        let mut response = [0u8; 25];
        client
            .read_exact(&mut response)
            .await
            .expect("read 100 Continue response");
        assert_eq!(&response, b"HTTP/1.1 100 Continue\r\n\r\n");
        client.write_all(b"body").await.expect("write body after continue");
    });

    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(true);
    ctx.request_info.in_request_body_stage = true;
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert_eq!(
        adapter.read_request_body(None).await.expect("expect body must buffer"),
        Bytes::from_static(b"body")
    );
    assert!(adapter.inner.req_header().headers.get(http::header::EXPECT).is_none());
    client_task.await.expect("expect client task must finish");
}

#[tokio::test]
async fn expect_continue_declared_over_physical_cap_rejects_without_interim() {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(b"POST /body-test HTTP/1.1\r\nHost: test\r\nContent-Length: 8\r\nExpect: 100-continue\r\n\r\n")
        .await
        .expect("write expect request headers");
    session.read_request().await.expect("read request headers");

    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(true);
    ctx.request_info.in_request_body_stage = true;
    ctx.request_body_config.max_body_size_bytes = 4;
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert_eq!(
        adapter.read_request_body(None).await,
        Err(BodyReadError::BufferLimitExceeded { limit: 4 })
    );

    let mut byte = [0u8; 1];
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), client.read(&mut byte))
            .await
            .is_err(),
        "the gateway must not send 100 Continue for a known-over-cap body"
    );
}

/// Regression coverage for `read_response_body`'s own wiring (the
/// override at the top of this file), mirroring
/// `read_request_body_refuses_once_past_request_stage` above: a bare
/// `response_admission` unit test (see the `response_admission_*` tests
/// earlier in this module) cannot catch a bug in how the override reads
/// `self.response_header` / `self.inner.req_header().method` or whether
/// it forwards the refusal to `ctx.record_error`. This drives the real
/// trait method through a `with_response_header`-bound adapter over a
/// real `pingora_proxy::Session`, status 101, the upgrade-refusal case.
#[tokio::test]
async fn read_response_body_refuses_upgrade_status() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    let mut response_header = ResponseHeader::build(101, None).unwrap();

    {
        let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
        let result = adapter.read_response_body(None).await;
        assert!(matches!(result, Err(BodyReadError::NotBuffered)));
    }
    assert!(ctx.diagnostics.contains_error(EdgionErrorCode::BodyRefusedWebsocket));
}

/// Stage-guard counterpart: `PingoraSessionAdapter::new` binds no
/// response header at all, which `response_admission` treats as "not at
/// the response stage" and refuses. Confirms the override still routes
/// through `response_admission`'s `None` arm and records the refusal.
#[tokio::test]
async fn read_response_body_refuses_without_bound_response_header() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();

    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        let result = adapter.read_response_body(None).await;
        assert!(matches!(result, Err(BodyReadError::NotBuffered)));
    }
    assert!(ctx.diagnostics.contains_error(EdgionErrorCode::BodyRefusedStage));
}

/// PassThrough path: a bodyless status (204) that the UPSTREAM itself sent —
/// both the bound header and `pre_plugin_response_status` agree — must return
/// an empty, non-error body rather than falling into the
/// `Buffer`/`Unimplemented` arm.
#[tokio::test]
async fn read_response_body_bodyless_status_passes_through_empty() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.pre_plugin_response_status = Some(204);
    let mut response_header = ResponseHeader::build(204, None).unwrap();

    let result = {
        let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
        adapter.read_response_body(None).await
    };
    assert_eq!(result.expect("204 must pass through"), Bytes::new());
}

/// Regression pin for the status half of the same defect shape as Finding 3:
/// the bound response header is response-stage mutable
/// (`set_response_status`, reachable from DSL's `resp.set_status` and from
/// `response_rewrite`), so a `resp.set_status(204)` ordered ahead of a body
/// consumer in the SAME `upstreamResponseFilterPlugins` array must not
/// fabricate the bodyless short-circuit — the upstream sent 200 and its body
/// is real and downstream-bound. Must stay on the `Buffer` arm.
#[tokio::test]
async fn read_response_body_ignores_plugin_rewritten_bodyless_status() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    // The upstream's own status, stamped in `upstream_response_filter`
    // before any response-stage plugin ran.
    ctx.pre_plugin_response_status = Some(200);
    let mut response_header = ResponseHeader::build(200, None).unwrap();

    let result = {
        let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
        adapter.set_response_status(204).expect("204 is a valid status");
        assert_eq!(adapter.response_header.as_ref().unwrap().status.as_u16(), 204);
        adapter.read_response_body(None).await
    };
    assert!(
        matches!(result, Err(BodyReadError::Unimplemented)),
        "a plugin-rewritten bodyless status must not downgrade the read to PassThrough"
    );
}

/// The mirror direction: the upstream really did send 204, a plugin rewrote
/// it to 200. There is no body either way, but the disagreement must still
/// steer to the loud `Buffer` rather than the silent `PassThrough` — the
/// same conservative choice the HEAD pair makes.
#[tokio::test]
async fn read_response_body_ignores_bodyless_snapshot_when_status_rewritten_away() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.pre_plugin_response_status = Some(204);
    let mut response_header = ResponseHeader::build(204, None).unwrap();

    let result = {
        let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
        adapter.set_response_status(200).expect("200 is a valid status");
        adapter.read_response_body(None).await
    };
    assert!(
        matches!(result, Err(BodyReadError::Unimplemented)),
        "a rewritten-away-from-bodyless response must not keep the short-circuit"
    );
}

/// Buffer path: an ordinary 200 on a non-HEAD, non-gRPC request must
/// reach the `Buffer` arm and fail with `Unimplemented` (real chunk
/// buffering is not wired up yet). the entry method is left missing, which derives
/// `false` for HEAD, so this also pins the `is_head` routing: an inverted
/// read would misroute this case to `PassThrough` instead.
#[tokio::test]
async fn read_response_body_ordinary_200_is_unimplemented() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    let mut response_header = ResponseHeader::build(200, None).unwrap();

    let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
    let result = adapter.read_response_body(None).await;
    assert!(matches!(result, Err(BodyReadError::Unimplemented)));
}

/// A genuine, unrewritten HEAD request still short-circuits to
/// `PassThrough`: both the snapshot and the live header say HEAD, so
/// closing the plugin-mutable holes must not break the RFC 9110 9.3.2 case
/// itself.
#[tokio::test]
async fn read_response_body_genuine_head_passes_through_empty() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.method = Some(http::Method::HEAD);
    session.req_header_mut().set_method(http::Method::HEAD);
    let mut response_header = ResponseHeader::build(200, None).unwrap();

    let result = {
        let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
        adapter.read_response_body(None).await
    };
    assert_eq!(result.expect("genuine HEAD must pass through"), Bytes::new());
}

/// Regression pin for Finding 3, rewrite-TO-HEAD direction: a plugin
/// calling `set_upstream_method("HEAD")` must not fabricate the bodyless
/// short-circuit. Reachable at the response stage too — DSL's
/// `req.set_method` is not stage-gated — where the upstream body has
/// already arrived. The read must stay on the `Buffer` arm
/// (`Unimplemented` today) rather than silently resolving to
/// `PassThrough` -> `Ok(empty)`, which a body-inspecting plugin reads as
/// "the response genuinely had no body".
#[tokio::test]
async fn read_response_body_ignores_plugin_rewritten_head_method() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    // The snapshot ran on the real downstream GET.
    ctx.request_info.method = Some(http::Method::GET);
    let mut response_header = ResponseHeader::build(200, None).unwrap();

    let result = {
        let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
        adapter.set_upstream_method("HEAD").expect("HEAD parses");
        assert_eq!(adapter.inner.req_header().method, http::Method::HEAD);
        adapter.read_response_body(None).await
    };
    assert!(
        matches!(result, Err(BodyReadError::Unimplemented)),
        "a plugin-rewritten HEAD must not downgrade the read to PassThrough"
    );
}

/// Regression pin for Finding 3, rewrite-AWAY-FROM-HEAD direction (the
/// mirror hole a snapshot-only read would open): the client sent HEAD, a
/// request-stage plugin rewrote the method to a body-carrying one, so the
/// upstream returns a real body AND pingora writes it downstream — its
/// `init_body_writer` keys on the same live header the rewrite mutated.
/// Trusting the snapshot alone here would hand a body consumer
/// `Ok(empty)` for a response that demonstrably has a body.
#[tokio::test]
async fn read_response_body_ignores_head_snapshot_when_method_rewritten_away() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    // The client really did send HEAD.
    ctx.request_info.method = Some(http::Method::HEAD);
    let mut response_header = ResponseHeader::build(200, None).unwrap();

    let result = {
        let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
        // A request-stage plugin (e.g. proxy_rewrite `method: GET`) rewrites
        // it, so the upstream response carries a body after all.
        adapter.set_upstream_method("GET").expect("GET parses");
        adapter.read_response_body(None).await
    };
    assert!(
        matches!(result, Err(BodyReadError::Unimplemented)),
        "a rewritten-away-from-HEAD request must not keep the bodyless short-circuit"
    );
}

/// Like `test_session`, but the caller supplies the whole request line +
/// headers. `apply_response_local_reply` keys on the request method and
/// (previously) on request headers, neither of which the fixed
/// `GET /dye-test` request can vary.
async fn test_session_raw(request: &str) -> Session {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(request.as_bytes())
        .await
        .expect("write request into duplex pair");
    drop(client);
    session
        .read_request()
        .await
        .expect("read_request should succeed")
        .then_some(())
        .expect("client must not have closed before sending the request");
    session
}

async fn apply_local_reply(request: &str, upstream_status: u16) -> (bool, EdgionHttpContext) {
    apply_local_reply_to_session(test_session_raw(request).await, upstream_status)
}

fn apply_local_reply_to_session(mut session: Session, upstream_status: u16) -> (bool, EdgionHttpContext) {
    let mut ctx = EdgionHttpContext::default();
    let mut response_header = ResponseHeader::build(upstream_status, None).unwrap();
    let applied = {
        let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
        adapter.apply_response_local_reply(403, Some(Bytes::from_static(b"denied")))
    };
    (applied, ctx)
}

async fn apply_connect_local_reply(upstream_status: u16) -> (bool, EdgionHttpContext) {
    let mut session = test_session().await;
    session.req_header_mut().set_method(http::Method::CONNECT);
    apply_local_reply_to_session(session, upstream_status)
}

/// The defect this refusal predicate was rewritten for: an `Upgrade` header
/// carrying a token nothing recognizes is not a protocol switch. The request
/// proxies as ordinary HTTP (only `websocket` is detected in
/// `pg_request_filter::detect_request_protocol`) and the upstream answers
/// 200, so a response-stage plugin's denial MUST be able to replace it.
/// Keying the refusal on the request header let any client append
/// `Upgrade: foo` and have the denial silently dropped — the response-stage
/// runner only logs when this returns false, and nothing there consults
/// the request-scoped plugin termination marker.
#[tokio::test]
async fn local_reply_replaces_response_for_unrecognized_upgrade_token() {
    let (applied, mut ctx) = apply_local_reply("GET /x HTTP/1.1\r\nHost: test\r\nUpgrade: foo\r\n\r\n", 200).await;
    assert!(applied, "an unrecognized Upgrade token must not block the denial");
    let (header, _) = ctx
        .take_pending_response_local_reply()
        .expect("accepted replacement must remain pending");
    assert_eq!(header.status.as_u16(), 403);
}

/// The genuine switch stays refused: the upstream answered 101, so the
/// exchange is no longer a replaceable HTTP response.
#[tokio::test]
async fn local_reply_refused_for_real_protocol_switch() {
    let (applied, ctx) = apply_local_reply(
        "GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
        101,
    )
    .await;
    assert!(!applied, "a real 101 must stay unreplaceable");
    assert!(!ctx.response_stage_settled());
}

/// Interim informational responses (100/103) are not the final response
/// there is nothing to replace yet.
#[tokio::test]
async fn local_reply_refused_for_interim_informational() {
    let (applied, _ctx) = apply_local_reply("GET /x HTTP/1.1\r\nHost: test\r\n\r\n", 103).await;
    assert!(!applied, "an interim 1xx must stay unreplaceable");
}

/// CONNECT is refused only once the tunnel actually exists.
#[tokio::test]
async fn local_reply_refused_for_established_connect_tunnel() {
    let (applied, _ctx) = apply_connect_local_reply(200).await;
    assert!(!applied, "an established CONNECT tunnel must stay unreplaceable");
}

/// ...and an upstream-rejected CONNECT never became a tunnel, so it is an
/// ordinary replaceable response.
#[tokio::test]
async fn local_reply_replaces_rejected_connect() {
    let (applied, mut ctx) = apply_connect_local_reply(403).await;
    assert!(applied, "a CONNECT the upstream refused carries no tunnel");
    let (header, _) = ctx
        .take_pending_response_local_reply()
        .expect("rejected CONNECT must carry the replacement reply");
    assert_eq!(header.status.as_u16(), 403);
}

async fn terminate_for(request: &str, upstream_status: u16) -> (bool, EdgionHttpContext) {
    use crate::plugins::runtime::plugin_runtime::check_response_stage_terminate;

    let mut session = test_session_raw(request).await;
    let mut ctx = EdgionHttpContext::default();
    // As captured by `upstream_response_filter` before plugin chains run.
    ctx.pre_plugin_response_status = Some(upstream_status);
    ctx.select_final_downstream_status(upstream_status);
    let mut response_header = ResponseHeader::build(upstream_status, None).unwrap();
    let result = PluginRunningResult::ErrResponse {
        status: 403,
        body: Some(Bytes::from_static(b"denied")),
    };
    let terminate = {
        let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
        let mut log = PluginLog::with_alias("dsl", None);
        check_response_stage_terminate(&result, &mut log, &mut adapter)
    };
    (terminate, ctx)
}

/// The termination half of the same defect: a refused local reply must ARM
/// the abort, not just log. Before this, the runner's `set_terminate` wrote
/// the plugin termination marker, whose readers are all request-stage, so the
/// response-stage entry point forwarded the upstream response and the denial
/// vanished. Driven on a real `101` — the case that stays unreplaceable even
/// after the refusal predicate was narrowed to response-side facts.
#[tokio::test]
async fn refused_local_reply_arms_the_response_stage_abort() {
    let (terminate, ctx) = terminate_for(
        "GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
        101,
    )
    .await;

    assert!(terminate);
    assert_eq!(
        ctx.response_denial(),
        Some(&ResponseStageDenial::Http(403)),
        "the denial must survive as an abort the entry point can act on"
    );
    assert!(ctx
        .diagnostics
        .contains_error(EdgionErrorCode::PluginResponseDenyAborted));
    assert_eq!(
        ctx.final_downstream_status(),
        Some(403),
        "the access log must show the denial, not the upstream response the client never got"
    );
}

/// The ordinary path is unchanged: a replaceable response stashes the reply
/// and never arms the abort (which would double-handle the same denial).
#[tokio::test]
async fn accepted_local_reply_does_not_arm_the_abort() {
    let (terminate, ctx) = terminate_for("GET /x HTTP/1.1\r\nHost: test\r\n\r\n", 200).await;

    assert!(terminate);
    assert!(ctx.response_stage_settled());
    assert_eq!(ctx.response_denial(), None);
    assert!(!ctx
        .diagnostics
        .contains_error(EdgionErrorCode::PluginResponseDenyAborted));
}

/// `ErrGrpcError` has no response-stage writer at all — the sync runner
/// cannot await `send_grpc_error_direct` — so before this it was a pure log
/// line and the upstream response was forwarded. The gRPC status must be
/// carried verbatim: deriving it from an HTTP status later would flatten
/// `RESOURCE_EXHAUSTED` (8) into `INTERNAL` (13).
#[tokio::test]
async fn response_stage_grpc_error_arms_the_abort_with_the_exact_status() {
    use crate::plugins::runtime::plugin_runtime::check_response_stage_terminate;

    let mut session = test_session_raw("POST /svc/M HTTP/1.1\r\nHost: test\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.is_grpc_request = true;
    ctx.pre_plugin_response_status = Some(200);
    ctx.select_final_downstream_status(200);
    let mut response_header = ResponseHeader::build(200, None).unwrap();
    let result = PluginRunningResult::ErrGrpcError {
        grpc_status: 8,
        message: Some("quota exceeded".to_string()),
    };

    let terminate = {
        let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
        let mut log = PluginLog::with_alias("dsl", None);
        check_response_stage_terminate(&result, &mut log, &mut adapter)
    };

    assert!(terminate);
    assert_eq!(
        ctx.response_denial(),
        Some(&ResponseStageDenial::Grpc {
            status: 8,
            message: Some("quota exceeded".to_string()),
        })
    );
    assert_eq!(ctx.request_info.grpc_status(), Some(8));
    assert_eq!(
        ctx.final_downstream_status(),
        Some(200),
        "a gRPC error is HTTP 200 on the wire"
    );
    assert!(ctx
        .diagnostics
        .contains_error(EdgionErrorCode::PluginResponseDenyAborted));
}

/// The broadest leak of the three: a naked `ErrTerminateRequest` needs NO
/// precondition to be dropped — the arm simply returned `true`, the runner
/// called `set_terminate`, and the upstream response was forwarded. Now it
/// mirrors the request stage's fallback contract
/// (`error_response::terminate_with_fallback`).
#[tokio::test]
async fn response_stage_naked_terminate_arms_the_abort() {
    use crate::plugins::runtime::plugin_runtime::check_response_stage_terminate;

    let mut session = test_session_raw("GET /x HTTP/1.1\r\nHost: test\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::default();
    ctx.pre_plugin_response_status = Some(200);
    ctx.select_final_downstream_status(200);
    let mut response_header = ResponseHeader::build(200, None).unwrap();
    let result = PluginRunningResult::ErrTerminateRequest;

    let mut log = PluginLog::with_alias("dsl", None);
    let terminate = {
        let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
        check_response_stage_terminate(&result, &mut log, &mut adapter)
    };

    assert!(terminate);
    assert_eq!(
        serde_json::to_value(&log).unwrap()["log"],
        serde_json::json!(["deny terminate"])
    );
    assert_eq!(ctx.response_denial(), Some(&ResponseStageDenial::Http(502)));
    assert!(ctx
        .diagnostics
        .contains_error(EdgionErrorCode::PluginTerminateWithoutResponse));
    assert!(ctx
        .diagnostics
        .contains_error(EdgionErrorCode::PluginResponseDenyAborted));
    assert_eq!(ctx.final_downstream_status(), Some(502));
}

#[tokio::test]
async fn streamed_handler_and_complete_capture_are_incompatible_in_both_orders() {
    for capture_first in [false, true] {
        let mut session = test_session_with_body(b"abc").await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = true;
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        if capture_first {
            assert_eq!(adapter.ensure_request_body(None).await.unwrap(), 3);
            assert!(!adapter.install_request_body_stream_handler(Box::new(NoopStreamHandler)));
        } else {
            assert!(adapter.install_request_body_stream_handler(Box::new(NoopStreamHandler)));
            assert!(matches!(
                adapter.ensure_request_body(None).await,
                Err(BodyReadError::NotBuffered)
            ));
            assert!(matches!(
                adapter.capture_request_body(None).await,
                Err(BodyReadError::NotBuffered)
            ));
            struct UnexpectedConsumer;
            impl RequestBodyChunkConsumer for UnexpectedConsumer {
                fn consume(&mut self, _chunk: &[u8]) {
                    panic!("incompatible capture must not read bytes");
                }
            }
            let mut consumer = UnexpectedConsumer;
            assert!(matches!(
                adapter.capture_request_body_with_consumer(None, &mut consumer).await,
                Err(BodyReadError::NotBuffered)
            ));
            assert_eq!(session.body_bytes_read(), 0);
            assert!(!ctx.request_body.has_cached_body());
        }
    }
}

#[tokio::test]
async fn streamed_handler_prevents_bodyless_mutation_owner_creation() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(false);
    ctx.request_info.in_request_body_stage = true;
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert!(adapter.install_request_body_stream_handler(Box::new(NoopStreamHandler)));
    assert!(matches!(
        adapter.overwrite_request_body(Bytes::from_static(b"created")).await,
        Err(BodyReadError::NotBuffered)
    ));
    assert!(!ctx.request_body.has_cached_body());
}

#[tokio::test]
async fn streamed_handler_refuses_uncached_transport_preread() {
    let mut session = test_session_with_body(b"abc").await;
    assert!(session.read_request_body().await.unwrap().is_some());
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.in_request_body_stage = true;
    assert!(!ctx.request_body.has_cached_body());
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert!(!adapter.install_request_body_stream_handler(Box::new(NoopStreamHandler)));
    assert!(!ctx.request_body_handlers_selected());
}

#[tokio::test]
async fn waf_capability_preserves_election_stage_and_freeze_before_native_rejection() {
    use crate::ctx::{WafLogMode, WafLogSource, WafPhase, WafPhaseClaim, WafSelection};

    let mut session = test_session_with_body(b"12345678").await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.body_declared = Some(true);
    let owner = Arc::new(());
    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert_eq!(
            adapter.waf().claim_phase(&owner, WafPhase::Request),
            WafPhaseClaim::Pending
        );
        assert_eq!(
            adapter.waf().install_body_limit(&owner, 4),
            Err(BodyReadError::NotBuffered)
        );
    }
    assert!(ctx.select_waf(Some(WafSelection::new(
        Arc::clone(&owner),
        WafLogSource::Route,
        0,
        WafLogMode::On,
    ))));
    ctx.request_body_config.max_body_size_bytes = 32;
    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert_eq!(
            adapter.waf().install_body_limit(&owner, 4),
            Err(BodyReadError::NotBuffered)
        );
        assert!(adapter.ctx.waf_body_hard_limit().is_none());
    }
    ctx.request_info.in_request_body_stage = true;
    // Capture and mutate through the canonical adapter first so this valid
    // snapshot below the cap can distinguish a later mutation freeze.
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert_eq!(adapter.ensure_request_body(None).await, Ok(8));
    assert_eq!(adapter.overwrite_request_body(Bytes::from_static(b"ok")).await, Ok(()));
    assert_eq!(
        adapter.waf().install_body_limit(&Arc::new(()), 4),
        Err(BodyReadError::NotBuffered)
    );
    assert!(adapter.ctx.waf_body_hard_limit().is_none());
    assert_eq!(
        adapter.waf().install_body_limit(&owner, 4),
        Err(BodyReadError::BufferLimitExceeded { limit: 4 })
    );
    assert_eq!(adapter.ctx.request_body_hard_limit(), 4);
    assert_eq!(
        adapter.ctx.overwrite_request_body(Bytes::from_static(b"new")),
        Err(BodyReadError::NotBuffered)
    );
    assert!(adapter.waf().take_transaction(&owner).is_none());
    assert_eq!(adapter.ctx.request_body_hard_limit(), 4);
}

#[tokio::test]
async fn waf_capability_rejected_install_keeps_resident_transaction() {
    use crate::ctx::{WafLogMode, WafLogSource, WafSelection};
    use edgion_waf::seclang::{EvaluationMode, RequestHead, RuleEngineMode, RuntimeLimits, Transaction};

    let transaction = |status| {
        let config =
            crate::plugins::http::waf::fixture::config(&format!("SecAction \"id:1,phase:1,deny,status:{status}\"\n"));
        let generation = Arc::new(config.compile_generation().expect("test WAF compiles"));
        Transaction::new_shared_with_modes(
            generation,
            RequestHead::new(
                b"GET / HTTP/1.1",
                b"GET",
                b"/",
                b"/",
                b"HTTP/1.1",
                b"192.0.2.1",
                b"request-1",
                &[],
            ),
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
            RuleEngineMode::On,
        )
        .expect("test transaction is valid")
    };
    let mut session = test_session_with_body(b"").await;
    let mut ctx = EdgionHttpContext::default();
    let owner = Arc::new(());
    assert!(ctx.select_waf(Some(WafSelection::new(
        Arc::clone(&owner),
        WafLogSource::Route,
        0,
        WafLogMode::On,
    ))));
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert!(adapter.waf().install_transaction(Arc::clone(&owner), transaction(403)));
    assert!(!adapter.waf().install_transaction(Arc::clone(&owner), transaction(409)));
    assert!(adapter.waf().take_transaction(&Arc::new(())).is_none());
    let mut resident = adapter
        .waf()
        .take_transaction(&owner)
        .expect("resident remains installed");
    assert_eq!(
        resident
            .execute_request_headers()
            .unwrap()
            .intervention()
            .unwrap()
            .status(),
        403
    );
}

#[tokio::test]
async fn terminal_grpc_adapter_paths_preserve_order_trace_and_status() {
    for exact in [true, false] {
        let (mut session, _client) = test_session_with_client().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.is_grpc_request = true;
        ctx.request_info.trace_id.initialize(&session.req_header().headers);
        let trace_id = ctx.request_info.trace_id.standard_trace_id().unwrap().to_owned();
        for op in [
            PendingHeaderOp::OverwriteIfExists {
                name: "server".into(),
                value: http::HeaderValue::from_static("unexpected"),
            },
            PendingHeaderOp::AddIfAbsent {
                name: "x-order".into(),
                value: http::HeaderValue::from_static("first"),
            },
            PendingHeaderOp::OverwriteIfExists {
                name: "x-order".into(),
                value: http::HeaderValue::from_static("last"),
            },
            PendingHeaderOp::Set {
                name: "x-trace-id".into(),
                value: http::HeaderValue::from_static("stale"),
            },
            PendingHeaderOp::Set {
                name: "content-type".into(),
                value: http::HeaderValue::from_static("text/plain"),
            },
            PendingHeaderOp::Append {
                name: "grpc-status".into(),
                value: http::HeaderValue::from_static("0"),
            },
            PendingHeaderOp::Append {
                name: "grpc-status".into(),
                value: http::HeaderValue::from_static("1"),
            },
            PendingHeaderOp::Set {
                name: "grpc-message".into(),
                value: http::HeaderValue::from_static("stale"),
            },
        ] {
            ctx.queue_pending_response_header_op(op);
        }
        {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            if exact {
                adapter.send_grpc_error_direct(8, None).await.unwrap();
            } else {
                adapter
                    .write_response_header(Box::new(ResponseHeader::build(404, None).unwrap()), false)
                    .await
                    .unwrap();
            }
            assert!(adapter.grpc_response_terminated);
            adapter
                .write_response_body(Some(Bytes::from_static(b"must not escape")), true)
                .await
                .unwrap();
        }
        let written = session.response_written().unwrap();
        assert_eq!(written.status.as_u16(), 200);
        assert_eq!(written.headers["grpc-status"], if exact { "8" } else { "5" });
        assert_eq!(written.headers.get_all("grpc-status").iter().count(), 1);
        if exact {
            assert!(!written.headers.contains_key("grpc-message"));
        } else {
            assert_eq!(written.headers["grpc-message"], "not found");
        }
        assert_eq!(written.headers["x-order"], "last");
        assert_eq!(written.headers["x-trace-id"], trace_id);
        assert_eq!(written.headers["content-type"], "application/grpc");
        assert!(!written.headers.contains_key("server"));
        assert!(!written.headers.contains_key("strict-transport-security"));
        assert!(!written.headers.contains_key("content-length"));
        assert!(!written.headers.contains_key("transfer-encoding"));
        assert!(ctx.pending_response_header_ops().is_empty());
        assert_eq!(ctx.request_info.grpc_status(), Some(if exact { 8 } else { 5 }));
    }
}

#[tokio::test]
async fn terminal_grpc_adapter_failure_keeps_outcome_and_duplicate_queue() {
    for exact in [true, false] {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.is_grpc_request = true;
        {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            let result = if exact {
                adapter.send_grpc_error_direct(8, None).await
            } else {
                adapter
                    .write_response_header(Box::new(ResponseHeader::build(404, None).unwrap()), false)
                    .await
            };
            assert!(result.is_err());
            assert!(adapter.delivery_failure_recorded);
            assert!(adapter.grpc_response_terminated);
            adapter
                .write_response_body(Some(Bytes::from_static(b"suppressed")), true)
                .await
                .unwrap();
            adapter.ctx.queue_pending_response_header_op(PendingHeaderOp::Set {
                name: "x-unconsumed".into(),
                value: http::HeaderValue::from_static("pending"),
            });
            let duplicate = if exact {
                adapter.send_grpc_error_direct(13, None).await
            } else {
                adapter
                    .write_response_header(Box::new(ResponseHeader::build(503, None).unwrap()), false)
                    .await
            };
            assert!(duplicate.is_err());
        }
        assert_eq!(ctx.final_downstream_status(), Some(200));
        assert_eq!(ctx.request_info.grpc_status(), Some(if exact { 8 } else { 5 }));
        assert_eq!(ctx.pending_response_header_ops().len(), 1);
        assert_eq!(ctx.diagnostics.entries().len(), 1);
        assert!(ctx.diagnostics.contains_error(EdgionErrorCode::ClientClosed));
    }
}

#[tokio::test]
async fn borrowed_mtls_conditions_ignore_unrelated_san_allocation_cost() {
    use crate::ctx::ClientCertInfo;
    use crate::plugins::runtime::conditions::{CompiledPluginConditions, EvaluationResult};
    use edgion_resources::resources::edgion_plugins::conditions::{
        Condition, ConditionSet, MatchCore, MtlsField, MtlsMatchCondition, PluginConditions, SanType,
    };

    for unrelated_sans in [0, 4096] {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        let mut sans = vec!["uri:spiffe://example.com/first".to_string()];
        sans.extend((0..unrelated_sans).map(|index| format!("unrelated-{index}.example.com")));
        ctx.request_info.install_tls_info(
            None,
            None,
            Some(ClientCertInfo {
                subject: "CN=alice".to_string(),
                cn: Some("alice".to_string()),
                sans,
                fingerprint: "ab:cd".to_string(),
            }),
            None,
            None,
        );
        let stored = ctx.request_info.client_cert_info().unwrap() as *const ClientCertInfo;
        let predicates = [
            (MtlsField::Cn, None, "alice"),
            (MtlsField::San, Some(SanType::Uri), "spiffe://example.com/first"),
        ]
        .map(|(field, san_type, value)| {
            let mut condition = MtlsMatchCondition {
                field,
                san_type,
                core: MatchCore {
                    values: Some(vec![value.to_string()]),
                    ..Default::default()
                },
                negate: false,
                descriptor: Box::from(""),
            };
            condition.compile().unwrap();
            CompiledPluginConditions::new(PluginConditions {
                skip: None,
                run: Some(ConditionSet::all_of(vec![Condition::MtlsMatch(condition)])),
            })
        });
        let adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert_eq!(adapter.client_cert_info().unwrap() as *const ClientCertInfo, stored);
        let (owned, clone_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
            std::hint::black_box(adapter.client_cert_info().cloned())
        });
        assert_eq!(owned.as_ref().unwrap().sans.len(), unrelated_sans + 1);
        assert!(clone_stats.calls >= unrelated_sans + 5);
        eprintln!(
            "mTLS owned certificate baseline {unrelated_sans} unrelated SANs: {} allocations, {} requested bytes",
            clone_stats.calls, clone_stats.requested_bytes
        );
        for condition in predicates {
            let (result, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
                std::hint::black_box(condition.evaluate(&adapter))
            });
            assert_eq!(result, EvaluationResult::Run);
            assert_eq!(stats.calls, 0);
            assert_eq!(stats.requested_bytes, 0);
        }
    }
}

#[tokio::test]
async fn borrowed_client_certificate_absent_without_exposed_record() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    assert!(PingoraSessionAdapter::new(&mut session, &mut ctx)
        .client_cert_info()
        .is_none());
    ctx.request_info.install_tls_info(None, None, None, None, None);
    assert!(PingoraSessionAdapter::new(&mut session, &mut ctx)
        .client_cert_info()
        .is_none());
}

#[tokio::test]
async fn read_response_body_missing_entry_method_cannot_claim_live_head() {
    let mut session = test_session().await;
    session.req_header_mut().set_method(http::Method::HEAD);
    let mut ctx = EdgionHttpContext::default();
    let mut response_header = ResponseHeader::build(200, None).unwrap();
    let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
    assert!(adapter.get_request_method().is_none());
    assert!(matches!(
        adapter.read_response_body(None).await,
        Err(BodyReadError::Unimplemented)
    ));
}

#[tokio::test]
async fn borrowed_query_conditions_use_frozen_decoded_cache_without_value_copies() {
    use crate::plugins::runtime::conditions::{CompiledPluginConditions, EvaluationResult};
    use edgion_resources::resources::edgion_plugins::conditions::{
        Condition, ConditionSet, KeyMatchCondition, MatchCore, PluginConditions,
    };

    let mut session =
        test_session_raw("GET /entry?x=hello+world&empty=&encoded=foo%26bar HTTP/1.1\r\nHost: example.com\r\n\r\n")
            .await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.entry_raw_query = Some("x=hello+world&empty=&encoded=foo%26bar".to_string());
    session.req_header_mut().set_uri("/before?x=rewritten".parse().unwrap());
    {
        let adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert_eq!(adapter.get_query_param_ref("x"), Some("hello world"));
        assert_eq!(adapter.get_query_param_ref("empty"), Some(""));
        assert_eq!(adapter.get_query_param_ref("encoded"), Some("foo&bar"));
        assert_eq!(adapter.get_query_param_ref("missing"), None);
        assert_eq!(adapter.get_query_param("empty"), Some(String::new()));
        assert_eq!(adapter.get_query_param("missing"), None);
    }
    let stored = ctx.cached_query().get("x").unwrap().as_ptr();
    let key = KeyGet::Query { name: "x".to_string() };
    let mut condition = KeyMatchCondition {
        key: key.clone(),
        core: MatchCore {
            values: Some(vec!["hello world".to_string()]),
            ..Default::default()
        },
        negate: false,
        descriptor: Box::from(""),
    };
    condition.compile().unwrap();
    let conditions = CompiledPluginConditions::new(PluginConditions {
        skip: None,
        run: Some(ConditionSet::all_of(vec![Condition::KeyMatch(condition)])),
    });
    session.req_header_mut().set_uri("/after?x=changed".parse().unwrap());
    let adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert_eq!(adapter.request_query(), Some("x=changed"));
    assert_eq!(adapter.get_query_param_ref("x").unwrap().as_ptr(), stored);
    let (owned, calls) = crate::test_allocation_counter::count_allocations(|| adapter.key_get(&key));
    assert_eq!(owned.as_deref(), Some("hello world"));
    assert_eq!(calls, 1);
    for _ in 0..2 {
        let (result, calls) = crate::test_allocation_counter::count_allocations(|| conditions.evaluate(&adapter));
        assert_eq!(result, EvaluationResult::Run);
        assert_eq!(calls, 0, "cached query conditions must borrow the stored value");
    }
}
