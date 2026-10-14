use super::*;
use crate::ai::conversion::chat_to_anthropic::AnthropicResponseProcessor;
use crate::ai::quota::{QuotaAccumulator, QuotaFinalStatus};
use crate::ai::{AiUsageShared, UsageExtractionProcessor};
use crate::ctx::ResponseStreamKind;
use crate::plugins::http::ext_proc::{test_response_processor, AttachedExtProc, TEST_RESPONSE_WINDOW_CAP};
use crate::plugins::runtime::traits::session::PluginSession;
use crate::plugins::runtime::traits::streaming_response_processor::{
    StreamingResponseEvent, StreamingResponseProcessor,
};
use crate::plugins::runtime::PluginRuntime;
use crate::routes::grpc::{GrpcBackendHandle, GrpcRouteRuleUnit, SelectedGrpcBackend};
use crate::routes::http::match_unit::{HttpRouteRuleUnit, MatchSpecificity};
use crate::{MatchInfo, Rsc};
use async_trait::async_trait;
use bytes::Bytes;
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::{
    GRPCBackendRef, GRPCRoute, GRPCRouteRule, HTTPBackendRef, HTTPRouteMatch, HTTPRouteRule, ResourceMeta,
};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

async fn run_stream_processors(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    body: &mut Option<bytes::Bytes>,
    end_of_stream: bool,
    sink: &mut ResponseBodySink,
) {
    ctx.freeze_response_processor_driver();
    super::run_stream_processors(session, ctx, body, end_of_stream, sink).await;
}

async fn run_stream_processors_event(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    body: &mut Option<bytes::Bytes>,
    event: StreamingResponseEvent,
    sink: &mut ResponseBodySink,
) {
    ctx.freeze_response_processor_driver();
    super::run_stream_processors_event(session, ctx, body, event, sink).await;
}

fn ctx_with_upstream() -> crate::EdgionHttpContext {
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.backend_context = Some(crate::BackendContext {
        source: None,
        rsc: None,
        service_key: None,
        service_kind: None,
        upstreams: Vec::new(),
        current_upstream_id: None,
        backend_ref_idx: None,
        effective_policy: None,
    });
    ctx.push_upstream(None, None);
    ctx
}

/// Minimal live Session (pingora's `Session::new_h1` over a duplex pair);
/// same pattern as `session_adapter.rs`'s test_session helper.
async fn test_session() -> Session {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(b"GET /sse-test HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .expect("write minimal request into duplex pair");
    drop(client);
    session
        .read_request()
        .await
        .expect("read_request should succeed")
        .then_some(())
        .expect("client must not have closed before sending the request");
    session
}

fn bandwidth_runtime(rate: &str) -> PluginRuntime {
    use edgion_resources::resources::edgion_plugins::{
        BandwidthLimitConfig, EdgionPlugin, UpstreamResponseBodyFilterEntry,
    };

    let mut runtime = PluginRuntime::new();
    let entry = UpstreamResponseBodyFilterEntry::new(EdgionPlugin::BandwidthLimit(BandwidthLimitConfig {
        rate: rate.to_string(),
        rate_bytes_per_second: None,
    }));
    assert!(runtime
        .add_from_upstream_response_body_filters(&[entry], "default")
        .is_empty());
    runtime
}

fn http_route_unit(runtime: Arc<PluginRuntime>) -> Arc<HttpRouteRuleUnit> {
    let matched = HTTPRouteMatch {
        path: None,
        headers: None,
        query_params: None,
        method: None,
    };
    Arc::new(HttpRouteRuleUnit {
        session_persistence_ttl_secs: None,
        resource_key: "default/http".to_string(),
        matched_info: MatchInfo {
            rsc: Rsc::new("HTTPRoute", "default", "http"),
            rule_id: 0,
            match_id: 0,
            sv: 0,
            m: matched,
            labels_meta: None,
            annotations_meta: None,
        },
        rule: Arc::new(HTTPRouteRule {
            name: None,
            resolved_terminal_route_uid: None,
            resolved_ai_admission: None,
            resolved_terminal_rule_identity: None,
            matches: None,
            filters: None,
            backend_refs: None,
            timeouts: None,
            retry: None,
            session_persistence: None,
            parsed_timeouts: None,
            parsed_retry: None,
            parsed_forward_raw_path: None,
            parsed_allow_non_idempotent_retry: None,
            parsed_mirror_tuning: None,
        }),
        route: None,
        path_regex: None,
        parent_refs: None,
        compiled_header_regexes: Vec::new(),
        compiled_query_regexes: Vec::new(),
        has_query_matchers: false,
        specificity: MatchSpecificity {
            has_method: false,
            header_count: 0,
            query_count: 0,
            creation_ts_ms: 0,
            route_key: "default/http".to_string(),
            rule_index: 0,
            match_index: 0,
        },
        metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::empty()),
        plugin_runtime: runtime,
        backends: Arc::from(Vec::new()),
        backend_finder: Arc::new(crate::lb::BackendSelector::new(vec![], vec![])),
    })
}

fn grpc_rule() -> GRPCRouteRule {
    GRPCRouteRule {
        name: None,
        matches: None,
        filters: None,
        backend_refs: None,
        timeouts: None,
        retry: None,
        session_persistence: None,
        parsed_timeouts: None,
        parsed_retry: None,
        parsed_allow_non_idempotent_retry: None,
    }
}

fn grpc_route_unit(runtime: Arc<PluginRuntime>) -> Arc<GrpcRouteRuleUnit> {
    let json = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "GRPCRoute",
        "metadata": { "namespace": "default", "name": "grpc" },
        "spec": {
            "resolvedStatusController": "edgion.io/gateway-controller", "parentRefs": [{ "name": "gateway", "kind": "Gateway" }],
            "rules": [{}]
        }
    });
    let mut route: GRPCRoute = crate::test_current_status::from_value(json).unwrap();
    route.pre_parse();
    let manager = crate::routes::grpc::GlobalGrpcRouteManagers::new();
    manager.full_set(&HashMap::from([("default/grpc".to_string(), Arc::new(route))]));
    let mut unit = manager
        .route_units_cache
        .lock()
        .get("default/grpc")
        .and_then(|units| units.first())
        .expect("minimal GRPCRoute must compile one unit")
        .as_ref()
        .clone();
    unit.plugin_runtime = runtime;
    Arc::new(unit)
}

fn backend_ref(name: &str) -> HTTPBackendRef {
    HTTPBackendRef {
        name: name.to_string(),
        namespace: None,
        port: Some(80),
        weight: None,
        group: None,
        kind: None,
        filters: None,
        backend_tls_policy: None,
        ref_denied: None,
    }
}

fn selected_http_backend(runtime: Arc<PluginRuntime>) -> crate::backends::SelectedHttpBackend {
    let compiled = crate::routes::http::match_unit::compile_http_backend_fixture(
        backend_ref("http-backend"),
        "default",
        0,
        runtime,
    );
    let logical = crate::backends::LogicalBackendHandle::new(Arc::from(vec![compiled]), 0).unwrap();
    crate::backends::SelectedHttpBackend::Invalid(logical)
}

fn selected_grpc_backend(runtime: Arc<PluginRuntime>) -> SelectedGrpcBackend {
    let mut rule = grpc_rule();
    rule.backend_refs = Some(vec![GRPCBackendRef {
        name: "grpc-backend".to_string(),
        namespace: None,
        port: Some(80),
        weight: None,
        group: None,
        kind: None,
        filters: None,
        backend_tls_policy: None,
        ref_denied: None,
    }]);
    let mut route: edgion_resources::GRPCRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "test-route", "namespace": "default"}, "spec": {}
    }))
    .unwrap();
    route.spec.rules = Some(vec![rule]);
    let mut compiled = GrpcRouteRuleUnit::compile_grpc_rule(&route, 0).unwrap().backends;
    Arc::get_mut(&mut compiled).unwrap()[0].plugin_runtime = runtime;
    let logical = GrpcBackendHandle::new(compiled, 0).unwrap();
    SelectedGrpcBackend::new(logical, crate::backends::policy::BackendTlsPolicyLookup::Absent).unwrap()
}

async fn read_only_delay(ctx: &mut EdgionHttpContext) -> Option<Duration> {
    let body = Some(Bytes::from(vec![b'x'; 1024]));
    let mut session = test_session().await;
    run_read_only_body_plugins(&mut session, ctx, &body, false)
}

#[tokio::test]
async fn grpc_route_preempts_http_route_and_http_route_is_the_fallback() {
    let mut ctx = EdgionHttpContext::default();
    ctx.route_unit = Some(http_route_unit(Arc::new(bandwidth_runtime("2048"))));
    assert_eq!(read_only_delay(&mut ctx).await, Some(Duration::from_millis(500)));

    ctx.grpc_route_unit = Some(grpc_route_unit(Arc::new(bandwidth_runtime("1024"))));
    assert_eq!(read_only_delay(&mut ctx).await, Some(Duration::from_secs(1)));
}

#[tokio::test]
async fn grpc_backend_preempts_http_backend_and_http_backend_is_the_fallback() {
    let mut ctx = EdgionHttpContext::default();
    ctx.selected_backend = Some(selected_http_backend(Arc::new(bandwidth_runtime("2048"))));
    assert_eq!(read_only_delay(&mut ctx).await, Some(Duration::from_millis(500)));

    ctx.selected_grpc_backend = Some(selected_grpc_backend(Arc::new(bandwidth_runtime("1024"))));
    assert_eq!(read_only_delay(&mut ctx).await, Some(Duration::from_secs(1)));
}

#[tokio::test]
async fn route_backend_gateway_and_global_delays_merge_to_the_maximum() {
    let mut ctx = EdgionHttpContext::default();
    ctx.route_unit = Some(http_route_unit(Arc::new(bandwidth_runtime("8192"))));
    ctx.selected_backend = Some(selected_http_backend(Arc::new(bandwidth_runtime("4096"))));
    ctx.install_resolved_gateway_plugins(vec![Arc::new(bandwidth_runtime("1024"))]);
    ctx.install_resolved_global_plugins(vec![Arc::new(bandwidth_runtime("2048"))]);

    assert_eq!(read_only_delay(&mut ctx).await, Some(Duration::from_secs(1)));
}

struct Uppercase;
#[async_trait]
impl StreamingResponseProcessor for Uppercase {
    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        _end_of_stream: bool,
        _stream_kind: ResponseStreamKind,
        _sink: &mut ResponseBodySink,
        _session: &mut dyn PluginSession,
    ) {
        if let Some(b) = chunk.take() {
            *chunk = Some(Bytes::from(b.to_ascii_uppercase()));
        }
    }
}

struct TypedTerminalRecorder {
    observed: Arc<Mutex<Vec<StreamingResponseEvent>>>,
}

#[async_trait]
impl StreamingResponseProcessor for TypedTerminalRecorder {
    async fn on_event(
        &mut self,
        _chunk: &mut Option<Bytes>,
        event: StreamingResponseEvent,
        _stream_kind: ResponseStreamKind,
        _sink: &mut ResponseBodySink,
        _session: &mut dyn PluginSession,
    ) {
        self.observed.lock().push(event);
    }

    async fn on_chunk(
        &mut self,
        _chunk: &mut Option<Bytes>,
        _end_of_stream: bool,
        _stream_kind: ResponseStreamKind,
        _sink: &mut ResponseBodySink,
        _session: &mut dyn PluginSession,
    ) {
        panic!("typed callback must not collapse into the legacy callback")
    }
}

#[tokio::test]
async fn pre_trailer_event_stays_typed_and_does_not_close_response_terminal() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let mut ctx = EdgionHttpContext::default();
    ctx.push_response_stream_processor(Box::new(TypedTerminalRecorder {
        observed: Arc::clone(&observed),
    }));
    let mut session = test_session().await;
    let mut body = None;
    let mut sink = ResponseBodySink::new();

    run_stream_processors_event(
        &mut session,
        &mut ctx,
        &mut body,
        StreamingResponseEvent::TerminalBeforeTrailers,
        &mut sink,
    )
    .await;

    assert_eq!(*observed.lock(), [StreamingResponseEvent::TerminalBeforeTrailers]);
    assert_eq!(
        ctx.response_terminal_state,
        crate::response::terminal::ResponseTerminalState::Open
    );
}

struct DropChunk;
struct PreTrailerOutput;

#[async_trait]
impl StreamingResponseProcessor for PreTrailerOutput {
    async fn on_event(
        &mut self,
        body: &mut Option<Bytes>,
        event: StreamingResponseEvent,
        _stream_kind: ResponseStreamKind,
        _sink: &mut ResponseBodySink,
        _session: &mut dyn PluginSession,
    ) {
        if event == StreamingResponseEvent::TerminalBeforeTrailers {
            *body = Some(Bytes::from(vec![b'x'; 1024]));
        }
    }

    async fn on_chunk(
        &mut self,
        _body: &mut Option<Bytes>,
        _end_of_stream: bool,
        _stream_kind: ResponseStreamKind,
        _sink: &mut ResponseBodySink,
        _session: &mut dyn PluginSession,
    ) {
        panic!("the typed terminal callback must be preserved")
    }
}

#[tokio::test]
async fn pre_trailer_generated_body_uses_scoped_maximum_delay_without_closing_eos() {
    let proxy = EdgionHttpProxy {
        listener_port: 0,
        is_https_listener: false,
        listener_key: edgion_resources::ListenerKey("unit-pre-trailer".into()),
        server_header_opts: Default::default(),
        access_logger: Arc::new(crate::observe::AccessLogger::new()),
        egc: crate::config::bind_gateway_config(None),
        enable_http2: false,
        preflight_handler: super::super::PreflightHandler,
        http_route_manager: Arc::new(crate::routes::http::routes_mgr::HttpPortRouteManager::new()),
        grpc_route_manager: Arc::new(crate::routes::grpc::GrpcPortRouteManager::new()),
    };
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.route_unit = Some(http_route_unit(Arc::new(bandwidth_runtime("8192"))));
    ctx.selected_backend = Some(selected_http_backend(Arc::new(bandwidth_runtime("4096"))));
    ctx.install_resolved_gateway_plugins(vec![Arc::new(bandwidth_runtime("1024"))]);
    ctx.install_resolved_global_plugins(vec![Arc::new(bandwidth_runtime("2048"))]);
    ctx.push_response_stream_processor(Box::new(PreTrailerOutput));
    ctx.freeze_response_processor_driver();
    let mut body = None;
    let mut sink = ResponseBodySink::new();
    let delay = upstream_response_body_filter_event(
        &proxy,
        &mut session,
        &mut body,
        UpstreamResponseBodyEvent::TerminalBeforeTrailers,
        &mut sink,
        &mut ctx,
    )
    .await
    .unwrap();
    assert_eq!(delay, Some(Duration::from_secs(1)));
    assert_eq!(body.as_ref().map(Bytes::len), Some(1024));
    assert!(!sink.is_terminated());
    assert_eq!(
        ctx.response_terminal_state,
        crate::response::terminal::ResponseTerminalState::Open
    );
}

#[async_trait]
impl StreamingResponseProcessor for DropChunk {
    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        _end_of_stream: bool,
        _stream_kind: ResponseStreamKind,
        _sink: &mut ResponseBodySink,
        _session: &mut dyn PluginSession,
    ) {
        *chunk = None;
    }
}

struct DropSseChunk;
#[async_trait]
impl StreamingResponseProcessor for DropSseChunk {
    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        _end_of_stream: bool,
        stream_kind: ResponseStreamKind,
        _sink: &mut ResponseBodySink,
        _session: &mut dyn PluginSession,
    ) {
        if stream_kind == ResponseStreamKind::Sse {
            *chunk = None;
        }
    }
}

struct HeaderMutator;
#[async_trait]
impl StreamingResponseProcessor for HeaderMutator {
    fn on_response_header(&mut self, response: &mut ResponseHeader) {
        response.set_status(http::StatusCode::SERVICE_UNAVAILABLE).unwrap();
        response.insert_header("content-type", "application/json").unwrap();
    }

    fn changes_body_length(&self) -> bool {
        true
    }

    async fn on_chunk(
        &mut self,
        _chunk: &mut Option<Bytes>,
        _end_of_stream: bool,
        _stream_kind: ResponseStreamKind,
        _sink: &mut ResponseBodySink,
        _session: &mut dyn PluginSession,
    ) {
    }
}

struct Injector;
#[async_trait]
impl StreamingResponseProcessor for Injector {
    fn changes_body_length(&self) -> bool {
        true
    }
    async fn on_chunk(
        &mut self,
        _chunk: &mut Option<Bytes>,
        _end_of_stream: bool,
        _stream_kind: ResponseStreamKind,
        sink: &mut ResponseBodySink,
        _session: &mut dyn PluginSession,
    ) {
        sink.push(Bytes::from_static(b"-injected")).unwrap();
    }
}

/// Records every byte it is shown, into a handle the test still owns.
struct SeesOnlyBody {
    saw: Arc<Mutex<Vec<u8>>>,
}

#[async_trait]
impl StreamingResponseProcessor for SeesOnlyBody {
    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        _end_of_stream: bool,
        _stream_kind: ResponseStreamKind,
        _sink: &mut ResponseBodySink,
        _session: &mut dyn PluginSession,
    ) {
        if let Some(bytes) = chunk.as_deref() {
            self.saw.lock().extend_from_slice(bytes);
        }
    }
}

struct Terminator;
#[async_trait]
impl StreamingResponseProcessor for Terminator {
    async fn on_chunk(
        &mut self,
        _chunk: &mut Option<Bytes>,
        _end_of_stream: bool,
        _stream_kind: ResponseStreamKind,
        sink: &mut ResponseBodySink,
        _session: &mut dyn PluginSession,
    ) {
        sink.terminate();
    }
}

/// Flips a shared flag if it ever runs, into a handle the test still owns.
struct MarkRan {
    ran: Arc<Mutex<bool>>,
}
#[async_trait]
impl StreamingResponseProcessor for MarkRan {
    async fn on_chunk(
        &mut self,
        _chunk: &mut Option<Bytes>,
        _end_of_stream: bool,
        _stream_kind: ResponseStreamKind,
        _sink: &mut ResponseBodySink,
        _session: &mut dyn PluginSession,
    ) {
        *self.ran.lock() = true;
    }
}

struct OrderMarker {
    label: &'static str,
    order: Arc<Mutex<Vec<&'static str>>>,
}

#[derive(Default)]
struct LifecycleCounts {
    ran: usize,
    preempted: usize,
    released: usize,
}

struct LifecycleMarker {
    counts: Arc<Mutex<LifecycleCounts>>,
    terminate: bool,
}

struct ClaimWithoutTerminate;

#[async_trait]
impl StreamingResponseProcessor for ClaimWithoutTerminate {
    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        _end_of_stream: bool,
        _stream_kind: ResponseStreamKind,
        _sink: &mut ResponseBodySink,
        session: &mut dyn PluginSession,
    ) {
        assert!(session.try_claim_response_terminal(ResponseTerminalCause::GuardrailRejected));
        *chunk = None;
    }
}

struct PendingLifecycle {
    released: Arc<Mutex<usize>>,
}

#[async_trait]
impl StreamingResponseProcessor for PendingLifecycle {
    fn release_inflight(&mut self) {
        *self.released.lock() += 1;
    }

    async fn on_chunk(
        &mut self,
        _chunk: &mut Option<Bytes>,
        _end_of_stream: bool,
        _stream_kind: ResponseStreamKind,
        _sink: &mut ResponseBodySink,
        _session: &mut dyn PluginSession,
    ) {
        std::future::pending::<()>().await;
    }
}

#[async_trait]
impl StreamingResponseProcessor for LifecycleMarker {
    fn on_terminal_preempted(&mut self, _winner: ResponseTerminalCause) {
        self.counts.lock().preempted += 1;
    }

    fn release_inflight(&mut self) {
        self.counts.lock().released += 1;
    }

    async fn on_chunk(
        &mut self,
        _chunk: &mut Option<Bytes>,
        _end_of_stream: bool,
        _stream_kind: ResponseStreamKind,
        sink: &mut ResponseBodySink,
        _session: &mut dyn PluginSession,
    ) {
        self.counts.lock().ran += 1;
        if self.terminate {
            sink.terminate();
        }
    }
}

#[async_trait]
impl StreamingResponseProcessor for OrderMarker {
    async fn on_chunk(
        &mut self,
        _chunk: &mut Option<Bytes>,
        _end_of_stream: bool,
        _stream_kind: ResponseStreamKind,
        _sink: &mut ResponseBodySink,
        _session: &mut dyn PluginSession,
    ) {
        self.order.lock().push(self.label);
    }
}

#[test]
fn header_callbacks_run_and_processors_survive() {
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.push_response_stream_processor(Box::new(HeaderMutator));
    let mut response = ResponseHeader::build(200, None).unwrap();

    run_stream_processor_headers(&mut response, &mut ctx);

    assert_eq!(response.status, http::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers.get("content-type").unwrap(), "application/json");
    assert_eq!(ctx.response_stream_processor_count(), 1);
}

#[test]
fn framing_guard_applies_only_to_body_mutators() {
    let mut response = ResponseHeader::build(200, None).unwrap();
    response.insert_header("content-length", "42").unwrap();
    let mut observational = crate::EdgionHttpContext::default();
    observational.push_response_stream_processor(Box::new(Uppercase));
    enforce_stream_processor_framing(&mut response, &observational);
    assert_eq!(response.headers.get("content-length").unwrap(), "42");

    let mut mutating = crate::EdgionHttpContext::default();
    mutating.push_response_stream_processor(Box::new(HeaderMutator));
    enforce_stream_processor_framing(&mut response, &mutating);
    assert!(response.headers.get("content-length").is_none());

    let mut reserved = crate::EdgionHttpContext::default();
    reserved.reserve_response_framing();
    let mut response = ResponseHeader::build(200, None).unwrap();
    response.insert_header("content-length", "42").unwrap();
    enforce_stream_processor_framing(&mut response, &reserved);
    assert!(response.headers.get("content-length").is_none());
}

#[tokio::test]
async fn processors_run_in_install_order_and_mutate_in_place() {
    let mut session = test_session().await;
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.push_response_stream_processor(Box::new(Uppercase));

    let mut body = Some(Bytes::from_static(b"data: hi\n\n"));
    let mut sink = ResponseBodySink::new();
    run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;
    assert_eq!(body.as_deref(), Some(b"DATA: HI\n\n".as_slice()));
    // Processors survive the call (they run once per chunk, not once per request).
    assert_eq!(ctx.response_stream_processor_count(), 1);
}

#[tokio::test]
async fn later_processor_sees_earlier_processors_output() {
    let mut session = test_session().await;
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.push_response_stream_processor(Box::new(Uppercase));
    ctx.push_response_stream_processor(Box::new(DropChunk));

    let mut body = Some(Bytes::from_static(b"payload"));
    let mut sink = ResponseBodySink::new();
    run_stream_processors(&mut session, &mut ctx, &mut body, true, &mut sink).await;
    assert!(body.is_none(), "second processor dropped the chunk");
}

#[tokio::test]
async fn empty_pipeline_is_a_no_op() {
    let mut session = test_session().await;
    let mut ctx = crate::EdgionHttpContext::default();
    let mut body = Some(Bytes::from_static(b"untouched"));
    let mut sink = ResponseBodySink::new();
    run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;
    assert_eq!(body.as_deref(), Some(b"untouched".as_slice()));
}

#[tokio::test]
async fn body_event_before_final_header_freeze_fails_closed() {
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    let mut body = Some(Bytes::from_static(b"must-not-pass"));
    let mut sink = ResponseBodySink::new();

    super::run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

    assert!(body.is_none());
    assert!(sink.is_terminated());
    assert!(!ctx.response_processors_frozen());
    assert!(ctx.response_processor_driver().is_none());
}

#[tokio::test]
async fn processor_observes_response_stream_kind() {
    let mut session = test_session().await;
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.push_response_stream_processor(Box::new(DropSseChunk));

    let mut body = Some(Bytes::from_static(b"ordinary"));
    let mut sink = ResponseBodySink::new();
    run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;
    assert_eq!(body.as_deref(), Some(b"ordinary".as_slice()));

    ctx.response_stream_kind = ResponseStreamKind::Sse;
    let mut body = Some(Bytes::from_static(b"data: event\n\n"));
    let mut sink = ResponseBodySink::new();
    run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;
    assert!(body.is_none(), "processor observed the SSE classification");
}

#[tokio::test]
async fn injected_content_is_not_visible_to_later_processors() {
    // Injector pushes to the sink; the processor after it must see only the
    // original body, never the injected bytes.
    let mut session = test_session().await;
    let saw = Arc::new(Mutex::new(Vec::new()));
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.push_response_stream_processor(Box::new(Injector));
    ctx.push_response_stream_processor(Box::new(SeesOnlyBody { saw: saw.clone() }));
    let mut sink = ResponseBodySink::new();
    let mut body = Some(Bytes::from_static(b"orig"));

    run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

    assert_eq!(sink.peek_extra(), &[Bytes::from_static(b"-injected")]);
    assert_eq!(
        saw.lock().as_slice(),
        b"orig",
        "the sink is an injection channel, not part of the transform chain"
    );
}

#[tokio::test]
async fn semantic_withholding_does_not_expose_empty_callbacks_to_ext_proc_slot() {
    let mut session = test_session().await;
    let ran = Arc::new(Mutex::new(false));
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.ensure_ai_state().semantic_guard_claimed = true;
    ctx.push_response_stream_processor(Box::new(MarkRan { ran: Arc::clone(&ran) }));
    let mut sink = ResponseBodySink::new();
    let mut body = None;

    run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

    assert!(!*ran.lock());
    assert!(!sink.is_terminated());
}

#[tokio::test]
async fn terminate_stops_the_chain_immediately() {
    // A processor that terminates must prevent every later processor from
    // running at all -- not merely from seeing further chunks.
    let mut session = test_session().await;
    let ran = Arc::new(Mutex::new(false));

    let mut ctx = crate::EdgionHttpContext::default();
    ctx.push_response_stream_processor(Box::new(Terminator));
    ctx.push_response_stream_processor(Box::new(MarkRan { ran: ran.clone() }));
    let mut sink = ResponseBodySink::new();
    let mut body = Some(Bytes::from_static(b"orig"));

    run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

    assert!(sink.is_terminated());
    assert!(!*ran.lock(), "processor after terminate must not run");
}

#[tokio::test]
async fn ai_processor_terminate_skips_the_entire_plain_chain() {
    // The AI processor slot runs before the plain `stream_processors`
    // chain. If it terminates, the plain chain must not run at all --
    // this is the `!sink.is_terminated()` guard the driver holds on its
    // own entry into that chain, not merely on later iterations of it.
    let mut session = test_session().await;
    let ran = Arc::new(Mutex::new(false));

    let mut ctx = crate::EdgionHttpContext::default();
    ctx.ensure_ai_state().stream_processor = Some(Box::new(Terminator));
    ctx.push_response_stream_processor(Box::new(MarkRan { ran: ran.clone() }));
    let mut sink = ResponseBodySink::new();
    let mut body = Some(Bytes::from_static(b"orig"));

    run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

    assert!(sink.is_terminated());
    assert!(
        !*ran.lock(),
        "the plain chain must not run once the AI processor terminated"
    );
}

#[tokio::test]
async fn ai_semantic_guard_and_generic_processors_are_distinct_ordered_slots() {
    let mut session = test_session().await;
    let order = Arc::new(Mutex::new(Vec::new()));
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.ensure_ai_state().stream_processor = Some(Box::new(OrderMarker {
        label: "ai",
        order: Arc::clone(&order),
    }));
    ctx.ensure_ai_state().semantic_processor = Some(Box::new(OrderMarker {
        label: "guardrail",
        order: Arc::clone(&order),
    }));
    ctx.push_response_stream_processor(Box::new(OrderMarker {
        label: "ext-proc",
        order: Arc::clone(&order),
    }));
    let mut body = Some(Bytes::from_static(b"data: event\n\n"));
    let mut sink = ResponseBodySink::new();

    run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

    assert_eq!(&*order.lock(), &["ai", "guardrail", "ext-proc"]);
    assert!(ctx.response_processors_frozen());
    assert!(ctx.ai_state().unwrap().stream_processor.is_none());
    assert!(ctx.ai_state().unwrap().semantic_processor.is_none());
    assert!(ctx.response_processor_driver().is_some());
    assert_eq!(ctx.response_stream_processor_count(), 1);
}

#[tokio::test]
async fn terminal_cleans_every_non_winner_once_including_earlier_processors() {
    let mut session = test_session().await;
    let first = Arc::new(Mutex::new(LifecycleCounts::default()));
    let winner = Arc::new(Mutex::new(LifecycleCounts::default()));
    let later = Arc::new(Mutex::new(LifecycleCounts::default()));
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.push_response_stream_processor(Box::new(LifecycleMarker {
        counts: Arc::clone(&first),
        terminate: false,
    }));
    ctx.push_response_stream_processor(Box::new(LifecycleMarker {
        counts: Arc::clone(&winner),
        terminate: true,
    }));
    ctx.push_response_stream_processor(Box::new(LifecycleMarker {
        counts: Arc::clone(&later),
        terminate: false,
    }));
    let mut body = Some(Bytes::from_static(b"data: event\n\n"));
    let mut sink = ResponseBodySink::new();

    run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

    let first_counts = first.lock();
    assert_eq!((first_counts.ran, first_counts.preempted), (1, 1));
    drop(first_counts);
    let winner_counts = winner.lock();
    assert_eq!((winner_counts.ran, winner_counts.preempted), (1, 0));
    drop(winner_counts);
    let later_counts = later.lock();
    assert_eq!((later_counts.ran, later_counts.preempted), (0, 1));
    drop(later_counts);
    assert!(matches!(
        ctx.response_terminal_state,
        crate::response::terminal::ResponseTerminalState::Closed {
            winner: Some(ResponseProcessorSlot::Ordinary(1)),
            close: ResponseClose::GatewayTerminate,
            ..
        }
    ));
}

#[tokio::test]
async fn real_quota_terminal_preempts_guardrail_and_ext_proc_slots() {
    let mut session = test_session().await;
    let shared = Arc::new(AiUsageShared::default());
    let quota = Arc::new(QuotaAccumulator::new(16));
    quota.finish(QuotaFinalStatus::OverLimit);
    let semantic = Arc::new(Mutex::new(LifecycleCounts::default()));
    let ordinary = Arc::new(Mutex::new(LifecycleCounts::default()));
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.response_stream_kind = ResponseStreamKind::Sse;
    ctx.ensure_ai_state().semantic_guard_claimed = true;
    ctx.ensure_ai_state().stream_processor = Some(Box::new(UsageExtractionProcessor::new_with_quota(
        Arc::clone(&shared),
        Some(quota),
        true,
        true,
    )));
    ctx.ensure_ai_state().semantic_processor = Some(Box::new(LifecycleMarker {
        counts: Arc::clone(&semantic),
        terminate: false,
    }));
    ctx.push_response_stream_processor(Box::new(LifecycleMarker {
        counts: Arc::clone(&ordinary),
        terminate: false,
    }));
    let mut body = Some(Bytes::from_static(
        b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"x\"}}]}\n\n",
    ));
    let original = body.clone();
    let mut sink = ResponseBodySink::new();

    run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

    assert_eq!(body, original, "quota preserves the current canonical chunk");
    assert!(sink.is_terminated());
    assert_eq!(sink.peek_extra().len(), 1);
    assert!(sink.peek_extra()[0]
        .windows(b"token_quota_exceeded".len())
        .any(|part| part == b"token_quota_exceeded"));
    {
        let counts = semantic.lock();
        assert_eq!((counts.ran, counts.preempted), (0, 1));
    }
    {
        let counts = ordinary.lock();
        assert_eq!((counts.ran, counts.preempted), (0, 1));
    }
    assert!(ctx.ai_state().unwrap().canonical_record_batch.is_none());
    assert!(shared.gateway_terminated());
    assert!(matches!(
        ctx.response_terminal_state,
        crate::response::terminal::ResponseTerminalState::Closed {
            winner: Some(ResponseProcessorSlot::Ai),
            cause: Some(ResponseTerminalCause::QuotaExceeded),
            close: ResponseClose::GatewayTerminate,
            ..
        }
    ));
}

fn anthropic_streaming_processor(quota: Option<Arc<QuotaAccumulator>>) -> AnthropicResponseProcessor {
    let mut processor = AnthropicResponseProcessor::new_with_quota(
        "claude-canonical".to_string(),
        true,
        false,
        1_785_772_800,
        Arc::new(AiUsageShared::default()),
        quota,
        true,
    );
    let mut header = ResponseHeader::build(200, None).unwrap();
    header.insert_header("content-type", "text/event-stream").unwrap();
    processor.on_response_header(&mut header);
    processor
}

#[tokio::test]
async fn anthropic_quota_winner_reclaims_translated_bytes_before_guardrail_cleanup() {
    let mut session = test_session().await;
    let quota = Arc::new(QuotaAccumulator::new(16));
    quota.finish(QuotaFinalStatus::OverLimit);
    let semantic = Arc::new(Mutex::new(LifecycleCounts::default()));
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.response_stream_kind = ResponseStreamKind::Sse;
    let state = ctx.ensure_ai_state();
    state.semantic_guard_claimed = true;
    state.stream_processor = Some(Box::new(anthropic_streaming_processor(Some(quota))));
    state.semantic_processor = Some(Box::new(LifecycleMarker {
        counts: Arc::clone(&semantic),
        terminate: false,
    }));
    let mut body = Some(Bytes::from_static(b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"type\":\"message\",\"id\":\"msg_1\",\"model\":\"claude-provider\",\"role\":\"assistant\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":3,\"output_tokens\":0}}}\n\n"));
    let mut sink = ResponseBodySink::new();

    run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

    let body = body.expect("translated current chunk must remain client-visible");
    assert!(body
        .windows(b"chat.completion.chunk".len())
        .any(|part| part == b"chat.completion.chunk"));
    assert!(!body
        .windows(b"claude-provider".len())
        .any(|part| part == b"claude-provider"));
    assert_eq!(sink.peek_extra().len(), 1);
    assert!(sink.is_terminated());
    assert!(ctx.ai_state().unwrap().canonical_record_batch.is_none());
    let counts = semantic.lock();
    assert_eq!((counts.ran, counts.preempted), (0, 1));
}

#[tokio::test]
async fn anthropic_translation_failure_keeps_only_fixed_error_and_preempts_guardrail() {
    let mut session = test_session().await;
    let semantic = Arc::new(Mutex::new(LifecycleCounts::default()));
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.response_stream_kind = ResponseStreamKind::Sse;
    let state = ctx.ensure_ai_state();
    state.semantic_guard_claimed = true;
    state.stream_processor = Some(Box::new(anthropic_streaming_processor(None)));
    state.semantic_processor = Some(Box::new(LifecycleMarker {
        counts: Arc::clone(&semantic),
        terminate: false,
    }));
    let mut body = Some(Bytes::from_static(
        b"event: error\ndata: {\"type\":\"error\",\"private\":\"provider-payload\"}\n\n",
    ));
    let mut sink = ResponseBodySink::new();

    run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

    let body = body.expect("fixed translation error must remain client-visible");
    assert!(body
        .windows(b"response_translation_failed".len())
        .any(|part| part == b"response_translation_failed"));
    assert!(!body
        .windows(b"provider-payload".len())
        .any(|part| part == b"provider-payload"));
    assert!(sink.peek_extra().is_empty());
    assert!(sink.is_terminated());
    assert!(ctx.ai_state().unwrap().canonical_record_batch.is_none());
    let counts = semantic.lock();
    assert_eq!((counts.ran, counts.preempted), (0, 1));
}

#[tokio::test]
async fn first_real_ext_proc_terminal_preempts_second_attached_owner() {
    let mut session = test_session().await;
    let first_owner = Arc::new(AttachedExtProc::test_lazy());
    let second_owner = Arc::new(AttachedExtProc::test_lazy());
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.push_response_stream_processor(test_response_processor(Arc::clone(&first_owner), false));
    ctx.push_response_stream_processor(test_response_processor(Arc::clone(&second_owner), false));
    let mut body = Some(Bytes::from(vec![b'x'; TEST_RESPONSE_WINDOW_CAP + 1]));
    let mut sink = ResponseBodySink::new();

    run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

    assert!(body.is_none());
    assert!(sink.is_terminated());
    assert_eq!(first_owner.test_state(), ("fail-close", "input-cap", true));
    assert_eq!(second_owner.test_state(), ("superseded", "processor-failure", true));
    assert!(matches!(
        ctx.response_terminal_state,
        crate::response::terminal::ResponseTerminalState::Closed {
            winner: Some(ResponseProcessorSlot::Ordinary(0)),
            cause: Some(ResponseTerminalCause::ProcessorFailure),
            ..
        }
    ));
}

#[tokio::test]
async fn driver_natural_eos_and_logging_are_idempotent_for_bodyless_output() {
    let mut session = test_session().await;
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.ensure_ai_state().stream_processor = Some(Box::new(UsageExtractionProcessor::new(Arc::new(
        AiUsageShared::default(),
    ))));
    let mut body = None;
    let mut sink = ResponseBodySink::new();

    run_stream_processors(&mut session, &mut ctx, &mut body, true, &mut sink).await;

    let expected = crate::response::terminal::ResponseTerminalState::Closed {
        winner: None,
        cause: None,
        emission: None,
        close: ResponseClose::NaturalEos,
    };
    assert_eq!(ctx.response_terminal_state, expected);
    assert!(sink.peek_extra().is_empty());
    assert!(!sink.is_terminated());
    super::super::pg_logging::finalize_response_terminal_for_logging(&mut ctx, None);
    super::super::pg_logging::finalize_response_terminal_for_logging(
        &mut ctx,
        Some(pingora_core::ErrorSource::Upstream),
    );
    assert_eq!(ctx.response_terminal_state, expected);
}

#[tokio::test]
async fn claimed_terminal_is_force_terminated_when_winner_returns_early() {
    let mut session = test_session().await;
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.ensure_ai_state().semantic_processor = Some(Box::new(ClaimWithoutTerminate));
    let mut body = Some(Bytes::from_static(b"rejected"));
    let mut sink = ResponseBodySink::new();

    run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink).await;

    assert!(body.is_none());
    assert!(sink.is_terminated());
    assert!(matches!(
        ctx.response_terminal_state,
        crate::response::terminal::ResponseTerminalState::Closed {
            winner: Some(ResponseProcessorSlot::Semantic),
            emission: Some(crate::response::terminal::TerminalEmission::NoEvent),
            close: ResponseClose::GatewayTerminate,
            ..
        }
    ));
}

#[tokio::test]
async fn cancelling_an_awaiting_driver_releases_the_owned_run_set() {
    let released = Arc::new(Mutex::new(0));
    let mut session = test_session().await;
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.push_response_stream_processor(Box::new(PendingLifecycle {
        released: Arc::clone(&released),
    }));
    let mut body = Some(Bytes::from_static(b"pending"));
    let mut sink = ResponseBodySink::new();

    let result = tokio::time::timeout(
        Duration::from_millis(10),
        run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink),
    )
    .await;

    assert!(result.is_err());
    assert_eq!(*released.lock(), 1);
    assert!(ctx.has_response_stream_processors());
}

#[tokio::test]
async fn cancelled_driver_lease_releases_all_inflight_owners() {
    let ai = Arc::new(Mutex::new(LifecycleCounts::default()));
    let semantic = Arc::new(Mutex::new(LifecycleCounts::default()));
    let ordinary = Arc::new(Mutex::new(LifecycleCounts::default()));
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.ensure_ai_state().stream_processor = Some(Box::new(LifecycleMarker {
        counts: Arc::clone(&ai),
        terminate: false,
    }));
    ctx.ensure_ai_state().semantic_processor = Some(Box::new(LifecycleMarker {
        counts: Arc::clone(&semantic),
        terminate: false,
    }));
    ctx.push_response_stream_processor(Box::new(LifecycleMarker {
        counts: Arc::clone(&ordinary),
        terminate: false,
    }));

    ctx.freeze_response_processor_driver();
    let driver = ctx.response_processor_driver().expect("frozen driver");
    drop(driver.body_lease().await.expect("body lease"));

    assert_eq!(ai.lock().released, 1);
    assert_eq!(semantic.lock().released, 1);
    assert_eq!(ordinary.lock().released, 1);
    assert!(ctx.ai_state().unwrap().stream_processor.is_none());
    assert!(ctx.ai_state().unwrap().semantic_processor.is_none());
    assert!(ctx.has_response_stream_processors());
}

#[tokio::test]
async fn guarded_physical_callback_limit_is_inclusive_and_stale_handoff_fails_closed() {
    let mut session = test_session().await;
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.ensure_ai_state().semantic_guard_claimed = true;
    let mut exact = Some(Bytes::from(vec![b'x'; MAX_CANONICAL_SOURCE_CHUNK_BYTES]));
    let mut sink = ResponseBodySink::new();
    run_stream_processors(&mut session, &mut ctx, &mut exact, false, &mut sink).await;
    assert_eq!(exact.as_ref().map(Bytes::len), Some(MAX_CANONICAL_SOURCE_CHUNK_BYTES));
    assert!(!sink.is_terminated());

    let mut plus_one = Some(Bytes::from(vec![b'x'; MAX_CANONICAL_SOURCE_CHUNK_BYTES + 1]));
    let mut sink = ResponseBodySink::new();
    run_stream_processors(&mut session, &mut ctx, &mut plus_one, false, &mut sink).await;
    assert!(plus_one.is_none());
    assert!(sink.is_terminated());

    let mut stale_ctx = crate::EdgionHttpContext::default();
    let state = stale_ctx.ensure_ai_state();
    state.semantic_guard_claimed = true;
    state.canonical_record_batch = Some(crate::ai::protocols::openai::canonical_sse::CanonicalRecordBatch::new());
    let mut body = None;
    let mut sink = ResponseBodySink::new();
    run_stream_processors(&mut session, &mut stale_ctx, &mut body, true, &mut sink).await;
    assert!(sink.is_terminated());
    assert!(stale_ctx.ai_state().unwrap().canonical_record_batch.is_none());
}

#[test]
fn ttft_requires_first_nonempty_body_chunk_and_never_overwrites() {
    let mut ctx = ctx_with_upstream();
    record_first_body_time(&mut ctx, None);
    record_first_body_time(&mut ctx, Some(&Bytes::new()));
    assert_eq!(ctx.get_current_upstream().and_then(|upstream| upstream.bt), None);

    record_first_body_time(&mut ctx, Some(&Bytes::from_static(b"x")));
    let first = ctx.get_current_upstream().and_then(|upstream| upstream.bt);
    assert!(first.is_some());

    ctx.get_current_upstream_mut().unwrap().start_time = Instant::now() - Duration::from_secs(1);
    record_first_body_time(&mut ctx, Some(&Bytes::from_static(b"later")));
    assert_eq!(ctx.get_current_upstream().and_then(|upstream| upstream.bt), first);
}

#[tokio::test]
async fn ip_dependency_abort_discards_queued_output_and_releases_real_conversion_budget_before_logging() {
    use crate::plugins::http::grpc_json_transcoder::GrpcJsonTranscoder;
    use base64::Engine;
    use edgion_resources::filters::PluginRunningResult;
    let mut session = test_session().await;
    session.req_header_mut().set_uri("/v1/hello/world".parse().unwrap());
    let mut ctx = EdgionHttpContext::default();
    ctx.route_unit = Some(http_route_unit(Arc::new(PluginRuntime::new())));
    ctx.request_info.normalized_current_path = "/v1/hello/world".into();
    ctx.request_info.body_declared = Some(false);
    ctx.request_info.in_request_body_stage = true;
    let config = serde_json::from_value(serde_json::json!({
        "descriptor":{"type":"inline","base64":base64::engine::general_purpose::STANDARD.encode(include_bytes!("../../../../../../edgion-tests/proto/grpc_json_transcoding/test.pb"))},
        "services":["test.TestService"]
    })).unwrap();
    let plugin = GrpcJsonTranscoder::create_inline(
        &config,
        edgion_resources::resources::edgion_plugins::BodyRequirement::fail_close(),
    )
    .unwrap();
    let mut log = crate::plugins::runtime::PluginLog::new("GrpcJsonTranscoder");
    {
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert_eq!(
            plugin.run_request(&mut adapter, &mut log).await,
            PluginRunningResult::Nothing,
            "{log:?}"
        );
    }
    let plan = ctx.grpc_transcoding.as_ref().unwrap().clone();
    assert!(
        plan.output(vec![b'x']).is_ok(),
        "the real conversion reservation is live"
    );
    ctx.freeze_response_processor_driver();
    assert!(ctx.claim_final_response(200));
    ctx.mark_ip_body_denied();
    let mut body = Some(Bytes::from_static(b"current-output"));
    let mut sink = ResponseBodySink::new();
    sink.push(Bytes::from_static(b"queued-output")).unwrap();
    assert!(!sink.peek_extra().is_empty());
    let error = abort_ip_body(&mut ctx, &mut body, &mut sink).await.unwrap_err();
    assert!(!error.retry());
    assert!(body.is_none());
    assert!(sink.peek_extra().is_empty());
    assert!(sink.is_terminated());
    assert!(
        plan.output(vec![b'y']).is_err(),
        "budget ownership is released before logging"
    );
    assert_eq!(ctx.final_downstream_status(), Some(200));
    assert!(ctx.response_write_started());
    assert!(matches!(
        ctx.response_terminal_state,
        crate::response::terminal::ResponseTerminalState::Closed {
            winner: None,
            close: ResponseClose::GatewayTerminate,
            ..
        }
    ));
}

#[tokio::test]
async fn ip_dependency_abort_preempts_all_driver_slots_without_a_synthetic_winner() {
    let owners: Vec<_> = (0..3)
        .map(|_| Arc::new(Mutex::new(LifecycleCounts::default())))
        .collect();
    let attached = Arc::new(AttachedExtProc::test_lazy());
    let mut ctx = EdgionHttpContext::default();
    let state = ctx.ensure_ai_state();
    state.stream_processor = Some(Box::new(LifecycleMarker {
        counts: owners[0].clone(),
        terminate: false,
    }));
    state.semantic_processor = Some(Box::new(LifecycleMarker {
        counts: owners[1].clone(),
        terminate: false,
    }));
    state.canonical_record_batch = Some(crate::ai::protocols::openai::canonical_sse::CanonicalRecordBatch::new());
    ctx.push_response_stream_processor(Box::new(LifecycleMarker {
        counts: owners[2].clone(),
        terminate: false,
    }));
    ctx.push_response_stream_processor(test_response_processor(attached.clone(), false));
    let trailers = Arc::new(Mutex::new(LifecycleCounts::default()));
    assert!(ctx.install_upstream_response_trailer(Box::new(IpAbortTrailer(trailers.clone()))));
    ctx.freeze_response_processor_driver();
    ctx.mark_ip_body_denied();
    let mut body = None;
    let mut sink = ResponseBodySink::new();
    sink.push(Bytes::from_static(b"extras-without-current-body")).unwrap();
    assert!(abort_ip_body(&mut ctx, &mut body, &mut sink).await.is_err());
    assert!(sink.peek_extra().is_empty());
    for owner in owners {
        let counts = owner.lock();
        assert_eq!((counts.preempted, counts.released), (1, 1));
    }
    assert!(
        attached.test_state().2,
        "the real attached owner is closed before logging"
    );
    assert!(ctx.ai_state().unwrap().canonical_record_batch.is_none());
    let counts = trailers.lock();
    assert_eq!((counts.ran, counts.preempted, counts.released), (0, 1, 1));
}

struct IpAbortTrailer(Arc<Mutex<LifecycleCounts>>);
#[async_trait]
impl crate::plugins::runtime::traits::UpstreamResponseTrailer for IpAbortTrailer {
    fn name(&self) -> &str {
        "IpAbortTrailer"
    }
    async fn run_upstream_response_trailer(
        &mut self,
        _: Option<&mut http::HeaderMap>,
        _: &mut dyn PluginSession,
        _: &mut crate::plugins::runtime::PluginLog,
    ) -> crate::plugins::runtime::traits::UpstreamTrailerResult {
        self.0.lock().ran += 1;
        crate::plugins::runtime::traits::UpstreamTrailerResult::Continue
    }
    fn on_terminal_preempted(&mut self, _: &'static str) {
        self.0.lock().preempted += 1;
    }
    fn release_inflight(&mut self) {
        self.0.lock().released += 1;
    }
}

#[tokio::test]
async fn unavailable_ip_body_event_aborts_pure_queued_output_before_any_trailer_dispatch() {
    let owner: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"body-gate","namespace":"b2-body-gate"},
        "spec":{"upstreamResponseBodyFilterPlugins":[{"type":"BandwidthLimit","config":{"rate":"1mb"},
            "conditions":{"run":{"allOf":[{"type":"directPeerIpMatch","ipRefs":[{"name":"never-published-b2-list"}],"negate":true}]}}
        }]}
    })).unwrap();
    let compiled = crate::plugins::http::compile_plugins(&owner);
    assert!(compiled.is_activatable, "{:?}", compiled.preparse_errors);
    for event in [
        UpstreamResponseBodyEvent::Data { end_of_stream: true },
        UpstreamResponseBodyEvent::TerminalWithoutTrailers,
        UpstreamResponseBodyEvent::TerminalBeforeTrailers,
    ] {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.route_unit = Some(http_route_unit(compiled.plugin_runtime.clone()));
        let trailers = Arc::new(Mutex::new(LifecycleCounts::default()));
        assert!(ctx.install_upstream_response_trailer(Box::new(IpAbortTrailer(trailers.clone()))));
        ctx.freeze_response_processor_driver();
        assert!(ctx.claim_final_response(200));
        let mut body = None;
        let mut sink = ResponseBodySink::new();
        sink.push(Bytes::from_static(b"queued-without-current-body")).unwrap();
        let error = process_response_body_event(&mut session, &mut body, event, &mut sink, &mut ctx)
            .await
            .unwrap_err();
        assert!(matches!(error.etype(), pingora_core::ErrorType::HTTPStatus(500)));
        assert!(!error.retry());
        assert!(ctx.ip_body_denied());
        assert!(body.is_none());
        assert!(sink.peek_extra().is_empty());
        assert!(sink.is_terminated());
        assert_eq!(ctx.final_downstream_status(), Some(200));
        let counts = trailers.lock();
        assert_eq!((counts.ran, counts.preempted, counts.released), (0, 1, 1));
    }
}
