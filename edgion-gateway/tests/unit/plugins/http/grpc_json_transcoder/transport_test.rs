use super::super::budget::Reservation;
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use edgion_resources::grpc_transcoding::GrpcJsonTranscoderConfig;
use pingora_core::upstreams::peer::{HttpPeer, Peer};
use tokio::sync::Semaphore;

fn context() -> EdgionHttpContext {
    let config: GrpcJsonTranscoderConfig = serde_json::from_value(serde_json::json!({
        "descriptor": {"type":"inline", "base64":STANDARD.encode(include_bytes!("../../../../../../edgion-tests/proto/grpc_json_transcoding/test.pb"))},
        "services":["test.TestService"]
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
