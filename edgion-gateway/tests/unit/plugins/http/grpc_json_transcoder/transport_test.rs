use super::super::budget::Reservation;
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use edgion_resources::grpc_transcoding::GrpcJsonTranscoderConfig;
use pingora_core::upstreams::peer::{HttpPeer, Peer};
use std::time::Duration;
use tokio::sync::Semaphore;

fn context() -> EdgionHttpContext {
    context_with_timeout("30s")
}
fn context_with_timeout(timeout: &str) -> EdgionHttpContext {
    let config: GrpcJsonTranscoderConfig = serde_json::from_value(serde_json::json!({
        "descriptor": {"type":"inline", "base64":STANDARD.encode(include_bytes!("../../../../../../edgion-tests/proto/grpc_json_transcoding/test.pb"))},
        "services":["test.TestService"], "timeout":timeout
    })).unwrap();
    let compiled = Arc::new(config.compile().unwrap());
    let binding = compiled.bind("GET", "/v1/hello/name", None).unwrap().binding;
    let mut ctx = EdgionHttpContext::default();
    let mut plan = GrpcTranscodingPlan::new(
        compiled,
        binding,
        Reservation::acquire_from(&Arc::new(Semaphore::new(1))).unwrap(),
        11,
    );
    plan.generation = ctx.request_body.generation();
    ctx.grpc_transcoding = Some(Arc::new(plan));
    ctx
}
#[test]
fn terminal_logging_preserves_outcomes_status_and_idempotence() {
    for (initial, expected, line) in [
        (Outcome::Pending, Outcome::Interrupted, "err: exchange interrupted"),
        (Outcome::Success, Outcome::Success, "exchange completed"),
        (Outcome::RpcStatus, Outcome::RpcStatus, "err: upstream status failed"),
        (
            Outcome::MissingStatus,
            Outcome::MissingStatus,
            "err: terminal status invalid",
        ),
        (
            Outcome::InvalidFraming,
            Outcome::InvalidFraming,
            "err: response framing invalid",
        ),
        (
            Outcome::InvalidMedia,
            Outcome::InvalidMedia,
            "err: response media invalid",
        ),
        (Outcome::Expired, Outcome::Expired, "err: exchange deadline expired"),
        (
            Outcome::OutputLimit,
            Outcome::OutputLimit,
            "err: output budget exhausted",
        ),
        (Outcome::Interrupted, Outcome::Interrupted, "err: exchange interrupted"),
        (
            Outcome::OwnerConflict,
            Outcome::OwnerConflict,
            "err: conversion owner conflict",
        ),
    ] {
        for already_logged in [false, true] {
            let mut ctx = context();
            let plan = ctx.grpc_transcoding.as_ref().unwrap().clone();
            plan.record(initial);
            plan.record_status(7);
            plan.record_status(13);
            plan.outcome.lock().logged = already_logged;
            ctx.stage_logs_mut().push(crate::plugins::runtime::log::StageLogs {
                stage: "request",
                filters: vec![crate::plugins::runtime::PluginLog::new("GrpcJsonTranscoder")],
                edgion_plugins: vec![],
            });
            plan.finalize_log(&mut ctx);
            plan.finalize_log(&mut ctx);
            let outcome = plan.outcome.lock();
            assert_eq!(outcome.kind, expected);
            assert_eq!(outcome.status, Some(7));
            assert!(outcome.logged);
            assert_eq!(ctx.request_info.grpc_status(), (!already_logged).then_some(7));
            assert_eq!(
                ctx.stage_logs_mut()[0].filters[0].render_for_test(),
                if already_logged { "" } else { line }
            );
        }
    }
}

#[test]
fn mandatory_h2_preserves_tls_domains_and_separates_ordinary_connections() {
    let ordinary = HttpPeer::new("127.0.0.1:443", true, "backend.example".into());
    let mut first = ordinary.clone();
    require_h2(&mut first);
    assert_eq!(first.options.alpn.to_string(), "H2");
    assert_ne!(first.reuse_hash(), ordinary.reuse_hash());
    assert_eq!(first.sni, ordinary.sni);
    let mut retry = ordinary.clone();
    require_h2(&mut retry);
    assert_eq!(first.reuse_hash(), retry.reuse_hash());
    retry = ordinary.clone();
    retry.group_key = 42;
    require_h2(&mut retry);
    assert_ne!(first.group_key, retry.group_key);
}
#[test]
fn attempt_headers_are_reapplied_and_h1_fallback_is_nonretryable() {
    let ctx = context();
    let mut req = RequestHeader::build("POST", b"/test.TestService/SayHello", None).unwrap();
    assert!(prepare_attempt(&mut req, &ctx).is_err());
    req.set_version(http::Version::HTTP_2);
    req.insert_header("content-encoding", "gzip").unwrap();
    req.insert_header("grpc-timeout", "99999999H").unwrap();
    prepare_attempt(&mut req, &ctx).unwrap();
    validate_attempt(&req, &ctx).unwrap();
    assert!(
        crate::routes::grpc::parse_grpc_timeout(req.headers["grpc-timeout"].to_str().unwrap()).unwrap()
            <= Duration::from_secs(30)
    );
    req.insert_header("te", "invalid").unwrap();
    assert!(validate_attempt(&req, &ctx).is_err());
    prepare_attempt(&mut req, &ctx).unwrap();
    validate_attempt(&req, &ctx).unwrap();
    ctx.grpc_transcoding.as_ref().unwrap().reject_mutation();
    assert!(prepare_attempt(&mut req, &ctx).is_err());
}
#[test]
fn late_method_and_query_mutations_fail_closed() {
    let ctx = context();
    for (method, uri) in [
        ("GET", "/test.TestService/SayHello"),
        ("POST", "/test.TestService/SayHello?name=override"),
        ("POST", "/other"),
    ] {
        let req = RequestHeader::build(method, uri.as_bytes(), None).unwrap();
        assert!(validate_request(&req, &ctx).is_err());
    }
    assert!(protected("GRPC-TIMEOUT"));
    assert!(!protected("authorization"));
}

struct ReplacingRequestHandler;
#[async_trait::async_trait]
impl crate::plugins::runtime::traits::RequestBodyStreamHandler for ReplacingRequestHandler {
    async fn on_chunk(
        &mut self,
        chunk: &mut Option<bytes::Bytes>,
        _: pingora_proxy::RequestBodyEvent,
    ) -> crate::plugins::runtime::traits::ChunkVerdict {
        *chunk = Some(bytes::Bytes::from_static(b"changed"));
        crate::plugins::runtime::traits::ChunkVerdict::Continue
    }
    fn take_log(&mut self) -> Option<crate::plugins::runtime::PluginLog> {
        None
    }
    fn dye_plan(&self) -> Option<Arc<crate::plugins::runtime::dye_plan::CompiledDyePlan>> {
        None
    }
}
#[test]
fn streamed_request_mutation_and_rpc_ownership_are_mutually_exclusive() {
    let mut earlier = context();
    let plan = earlier.grpc_transcoding.take().unwrap();
    assert!(earlier.push_request_body_stream_handler(Box::new(ReplacingRequestHandler)));
    earlier.grpc_transcoding = Some(plan);
    let req = RequestHeader::build("POST", b"/test.TestService/SayHello", None).unwrap();
    assert!(validate_request(&req, &earlier).is_err());

    let mut later = context();
    assert!(!later.has_request_body_plugins());
    assert!(!later.push_request_body_stream_handler(Box::new(ReplacingRequestHandler)));
    assert!(!later.has_request_body_plugins());
    assert!(!later.has_request_body_stream_handlers());
    assert!(later
        .grpc_transcoding
        .as_ref()
        .unwrap()
        .conflict
        .load(Ordering::Acquire));
    // Ignoring the refusal (for example ExtProc fail-open) cannot clear the RPC conflict.
    assert!(validate_request(&req, &later).is_err());
}

#[tokio::test]
async fn correction_extproc_body_ownership_and_registration_are_sticky() {
    use crate::plugins::http::ext_proc::test_body_owner;
    use crate::plugins::http::grpc_json_transcoder::GrpcResponseProcessor;
    use crate::plugins::runtime::{PingoraSessionAdapter, PluginSession};
    for body in [false, true] {
        let mut earlier = context();
        earlier.grpc_transcoding.take();
        assert!(earlier.install_attached_ext_proc("earlier".into(), Arc::new(test_body_owner(body)), body));
        assert_eq!(earlier.grpc_response_owner_compatible(false), !body);
        assert!(earlier.grpc_response_owner_compatible(true));
        let mut later = context();
        assert_eq!(
            later.install_attached_ext_proc("later".into(), Arc::new(test_body_owner(body)), body),
            !body
        );
        assert_eq!(
            later
                .grpc_transcoding
                .as_ref()
                .unwrap()
                .conflict
                .load(Ordering::Acquire),
            body
        );
    }
    let mut ctx = context();
    let plan = ctx.grpc_transcoding.clone().unwrap();
    let (_, stream) = tokio::io::duplex(4096);
    let mut session = pingora_proxy::Session::new_h1(Box::new(stream));
    let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert!(!adapter.install_attached_response_processor(Box::new(GrpcResponseProcessor::new(plan.clone()))));
    assert!(plan.conflict.load(Ordering::Acquire));
    assert!(!ctx.has_response_stream_processors());
    let mut before = context();
    let plan = before.grpc_transcoding.take().unwrap();
    assert!(before.push_response_stream_processor(Box::new(GrpcResponseProcessor::new(plan))));
    assert!(!before.grpc_response_owner_compatible(false));
}

#[test]
fn correction_scoped_body_consumers_preserve_header_only_targets() {
    use crate::plugins::runtime::PluginRuntime;
    use edgion_resources::resources::edgion_plugins::EdgionPlugins;
    fn runtime(consumer: u8) -> Arc<PluginRuntime> {
        let spec = if consumer == 3 {
            serde_json::json!({"upstreamResponseFilterPlugins":[{"type":"Wasm","config":{"source":{"file":concat!(env!("CARGO_MANIFEST_DIR"),"/../edgion-tests/wasm/artifacts/header_demo.wasm")},"pluginConfig":{"string":"header-only-transport-probe"}}}]})
        } else if consumer == 2 {
            serde_json::json!({"upstreamResponseBodyFilterPlugins":[{"type":"Wasm","config":{"source":{"file":concat!(env!("CARGO_MANIFEST_DIR"),"/../edgion-tests/wasm/artifacts/header_demo.wasm")},"pluginConfig":{"string":"response-body-probe"}}}]})
        } else if consumer == 1 {
            serde_json::json!({"upstreamResponseBodyFilterPlugins":[{"type":"BandwidthLimit","config":{"rate":"100kb"}}]})
        } else {
            serde_json::json!({"upstreamResponseFilterPlugins":[{"type":"Dsl","config":{"name":"header-companion","source":"resp.set_header(\"x-test\", \"ok\")"}}]})
        };
        let plugins: EdgionPlugins=serde_json::from_value(serde_json::json!({"apiVersion":"edgion.io/v1","kind":"EdgionPlugins","metadata":{"name":"companion","namespace":"default"},"spec":spec})).unwrap();
        crate::plugins::http::plugin_generation::PluginGeneration::new(plugins)
            .compile(None)
            .plugin_runtime
            .clone()
    }
    for scope in 0..3 {
        for consumer in 0..4 {
            let mut ctx = context();
            ctx.grpc_transcoding.take();
            let r = runtime(consumer);
            assert!(!r.rejects_invalid_config());
            match scope {
                0 => ctx.install_resolved_global_plugins(vec![r]),
                1 => ctx.install_resolved_gateway_plugins(vec![r]),
                _ => ctx.install_resolved_route_ext_refs(vec![Some(r)]),
            }
            assert_eq!(
                ctx.grpc_response_owner_compatible(false),
                consumer == 0 || consumer == 3
            );
            assert!(ctx.grpc_response_owner_compatible(true));
        }
    }
}

#[test]
fn correction_backend_checks_actual_pinned_target_not_wrapper_shape() {
    use crate::plugins::runtime::PluginRuntime;
    use edgion_resources::resources::edgion_plugins::EdgionPlugins;
    let filters = serde_json::from_value::<Vec<edgion_resources::HTTPRouteFilter>>(serde_json::json!([
        {"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"companion"}}
    ]))
    .unwrap();
    let owner: edgion_resources::HTTPRoute = serde_json::from_value(serde_json::json!({"apiVersion":"gateway.networking.k8s.io/v1", "kind":"HTTPRoute", "metadata":{"namespace":"default","name":"test-route"}, "spec":{"rules":[{"backendRefs":[{"name":"backend","port":80,"filters":filters}]}]}})).unwrap();
    let outer = PluginRuntime::from_httproute_filters_for_backend(&owner, 0, 0, &Default::default());
    assert!(!outer.has_ai_conversion_owner());
    assert_eq!(outer.upstream_response_body_plugins_count(), 1);
    for (spec, compatible) in [
        (
            serde_json::json!({"requestPlugins":[{"type":"AiProxy","config":{"api":"openaiChat"}}]}),
            false,
        ),
        (
            serde_json::json!({"requestPlugins":[{"type":"AiGuard","config":{"rules":[{"name":"tripwire","match":{"type":"regex","pattern":"private"},"action":"reject"}]}}]}),
            false,
        ),
        (
            serde_json::json!({"requestPlugins":[{"type":"Guardrail","config":{"target":{"name":"guard","port":8080},"request":{"path":{"template":"/decision"}}}}]}),
            false,
        ),
        (
            serde_json::json!({"upstreamResponseBodyFilterPlugins":[{"type":"BandwidthLimit","config":{"rate":"100kb"}}]}),
            false,
        ),
        (
            serde_json::json!({"upstreamResponseFilterPlugins":[{"type":"Wasm","config":{"source":{"file":concat!(env!("CARGO_MANIFEST_DIR"),"/../edgion-tests/wasm/artifacts/header_demo.wasm")},"pluginConfig":{"string":"backend-header-transport-probe"}}}]}),
            true,
        ),
        (
            serde_json::json!({"upstreamResponseFilterPlugins":[{"type":"Dsl","config":{"name":"header-companion","source":"resp.set_header(\"x-test\", \"ok\")"}}]}),
            true,
        ),
    ] {
        let plugins:EdgionPlugins=serde_json::from_value(serde_json::json!({"apiVersion":"edgion.io/v1","kind":"EdgionPlugins","metadata":{"name":"companion","namespace":"default"},"spec":spec})).unwrap();
        let target = crate::plugins::http::plugin_generation::PluginGeneration::new(plugins)
            .compile(None)
            .plugin_runtime
            .clone();
        assert!(!target.rejects_invalid_config());
        let mut ctx = context();
        ctx.pinned_backend_ext_ref("default/companion", || Some(target.clone()));
        let empty = Arc::new(PluginRuntime::new());
        let still_pinned = ctx.pinned_backend_ext_ref("default/companion", || Some(empty));
        assert!(Arc::ptr_eq(still_pinned.as_ref().unwrap(), &target));
        assert_eq!(outer.backend_compatible(&mut ctx), compatible);
        assert_eq!(
            ctx.grpc_transcoding.as_ref().unwrap().conflict.load(Ordering::Acquire),
            !compatible
        );
    }
}

#[test]
fn correction_ai_installation_and_outgoing_attempt_defenses_are_sticky() {
    use crate::ai::{AiAttemptPlan, AiPlanError};
    let mut ctx = context();
    assert!(!ctx.try_set_ai_attempt_plan(AiAttemptPlan::Rejected(AiPlanError::InvalidBackend)));
    assert!(ctx.ai_state().is_none());
    assert_eq!(
        ctx.grpc_transcoding.as_ref().unwrap().outcome.lock().kind,
        Outcome::OwnerConflict
    );
    let req = RequestHeader::build("POST", b"/test.TestService/SayHello", None).unwrap();
    let mut ctx = context();
    ctx.ensure_ai_state();
    assert!(validate_request(&req, &ctx).is_err());
    assert_eq!(
        ctx.grpc_transcoding.as_ref().unwrap().outcome.lock().kind,
        Outcome::OwnerConflict
    );
    let mut ctx = context();
    let plan = Arc::get_mut(ctx.grpc_transcoding.as_mut().unwrap()).unwrap();
    plan.deadline = Instant::now() - Duration::from_secs(1);
    assert!(validate_request(&req, &ctx).is_err());
    assert_eq!(
        ctx.grpc_transcoding.as_ref().unwrap().outcome.lock().kind,
        Outcome::Expired
    );
}

#[test]
fn unary_head_template_limits_preserve_boundaries_and_publication() {
    let check = |header: ResponseHeader, expected: bool| {
        let ctx = context();
        let plan = ctx.grpc_transcoding.as_ref().unwrap();
        assert!(!plan.streaming());
        let result = plan.head_plan(ResponseHeadSource::Origin, &header);
        assert_eq!(result.is_ok(), expected);
        match result {
            Ok(_) => {
                let template = plan.template.lock();
                let template = template.as_ref().unwrap();
                assert_eq!(template.status, header.status);
                assert_eq!(template.headers, header.headers);
            }
            Err(error) => {
                assert_eq!(error.etype(), &pingora_core::ErrorType::HTTPStatus(502));
                assert!(!error.retry());
                assert!(plan.template.lock().is_none());
            }
        }
        assert_eq!(plan.outcome.lock().kind, Outcome::Pending);
    };
    for repeated in [false, true] {
        for fields in [1, 63, 64, 65, 128] {
            let mut header = ResponseHeader::build(200, None).unwrap();
            header.insert_header("content-type", "application/json").unwrap();
            for index in 1..fields {
                let name = if repeated {
                    "x-extra".to_owned()
                } else {
                    format!("x-extra-{index}")
                };
                header.append_header(name, "value").unwrap();
            }
            assert_eq!(header.headers.len(), fields);
            check(header, fields <= 64);
        }
    }
    for bytes in [8191, 8192, 8193, 16384] {
        let mut header = ResponseHeader::build(200, None).unwrap();
        header.insert_header("content-type", "application/json").unwrap();
        let base = "content-type".len() + "application/json".len() + 4 + "x-pad".len() + 4;
        header.insert_header("x-pad", "a".repeat(bytes - base)).unwrap();
        assert_eq!(header.headers.len(), 2);
        check(header, bytes <= 8192);
    }
}

#[tokio::test]
async fn string_request_append_preserves_sealed_policy_and_parse_order() {
    use crate::plugins::runtime::session_adapter::{record_outcome_tests::test_session, PingoraSessionAdapter};
    use crate::plugins::runtime::traits::PluginSession;

    for sealed in [false, true] {
        for name in ["CoNtEnT-TyPe", "x-ordinary", "invalid name"] {
            for value in ["next", "bad\rvalue", "bad\nvalue"] {
                let mut session = test_session().await;
                let mut ctx = if sealed {
                    context()
                } else {
                    EdgionHttpContext::default()
                };
                session.req_header_mut().append_header("content-type", "first").unwrap();
                session.req_header_mut().append_header("x-ordinary", "first").unwrap();
                let before = session.req_header().headers.clone();
                let result = PingoraSessionAdapter::new(&mut session, &mut ctx).append_request_header(name, value);
                let blocked = sealed && name == "CoNtEnT-TyPe";
                if blocked {
                    assert_eq!(result.unwrap_err().to_string(), "sealed RPC header");
                } else if value != "next" {
                    assert_eq!(
                        result.unwrap_err().to_string(),
                        http::HeaderValue::from_str(value).unwrap_err().to_string()
                    );
                } else if name == "invalid name" {
                    assert!(result.is_err());
                } else {
                    result.unwrap();
                    let values: Vec<_> = session.req_header().headers.get_all(name).iter().collect();
                    assert_eq!(values.len(), 2);
                    assert_eq!(values[0], "first");
                    assert_eq!(values[1], "next");
                }
                if blocked || value != "next" || name == "invalid name" {
                    assert_eq!(session.req_header().headers, before);
                }
                if let Some(plan) = &ctx.grpc_transcoding {
                    assert_eq!(plan.conflict.load(Ordering::Acquire), blocked);
                    assert_eq!(
                        plan.outcome.lock().kind,
                        if blocked {
                            Outcome::OwnerConflict
                        } else {
                            Outcome::Pending
                        }
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn query_strip_only_rejects_a_real_sealed_mutation() {
    use crate::plugins::runtime::session_adapter::{record_outcome_tests::test_session, PingoraSessionAdapter};
    use crate::plugins::runtime::traits::PluginSession;
    for present in [false, true] {
        let mut session = test_session().await;
        let mut ctx = context();
        ctx.request_query
            .capture(present.then(|| "api%5Fkey=secret".into()))
            .unwrap();
        let result = PingoraSessionAdapter::new(&mut session, &mut ctx).remove_request_query_param("api_key");
        if present {
            assert!(result.is_err());
        } else {
            assert!(!result.unwrap());
        }
        assert_eq!(
            ctx.grpc_transcoding.as_ref().unwrap().conflict.load(Ordering::Acquire),
            present
        );
        assert!(!ctx.request_query.projection_is_dirty());
    }
}

#[test]
fn configured_budget_controls_owner_and_upstream_timeout() {
    let ctx = context_with_timeout("250ms");
    let plan = ctx.grpc_transcoding.as_ref().unwrap();
    assert!(plan.deadline > Instant::now());
    assert!(plan.deadline.duration_since(Instant::now()) <= Duration::from_millis(250));
    let mut req = RequestHeader::build("POST", b"/test.TestService/SayHello", None).unwrap();
    req.set_version(http::Version::HTTP_2);
    prepare_attempt(&mut req, &ctx).unwrap();
    let timeout = crate::routes::grpc::parse_grpc_timeout(req.headers["grpc-timeout"].to_str().unwrap()).unwrap();
    assert!(timeout <= Duration::from_millis(250));
}
