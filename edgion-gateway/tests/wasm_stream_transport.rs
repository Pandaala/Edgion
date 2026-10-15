//! Real downstream H1/H2 to H1-origin streaming, lifecycle and capacity proof.
//! Standalone exception: processed fixtures, ephemeral listeners, no shared binaries.
#![cfg(feature = "test-util")]

use async_trait::async_trait;
use base64::Engine as _;
use bytes::Bytes;
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_gateway::{routes::http::proxy_http::EdgionHttpProxy, EdgionHttpContext};
use parking_lot::Mutex;
use pingora_core::{
    apps::ServerApp,
    modules::http::HttpModules,
    protocols::{l4, Digest},
    upstreams::peer::HttpPeer,
    Error,
};
use pingora_http::ResponseHeader;
use pingora_proxy::{
    FailToProxy, ProxyHttp, RequestBodyEvent, ResponseBodySink, ResponseHeadBoundary, ResponseHeadBoundaryAction,
    ResponseHeadCommitPlan, ResponseHeadOutcome, ResponseHeadSource, ResponseHeadUsage, Session,
    UpstreamResponseBodyEvent,
};
use sha2::{Digest as _, Sha256};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

use edgion_gateway::ctx::ResponseStreamKind;
use edgion_gateway::plugins::runtime::{
    session_adapter::PingoraSessionAdapter,
    traits::{PluginSession, StreamingResponseProcessor},
};
use edgion_resources::{EdgionPlugins, Gateway, GatewayClass, HTTPRoute, ListenerKey, ResourceMeta};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, watch, Notify},
    task::JoinSet,
};
mod proxy {
    //! Full lifecycle delegation for the real proxy test.
    use super::*;
    pub(super) struct DelegatingProxy {
        pub(super) inner: EdgionHttpProxy,
        pub(super) gates: Arc<Mutex<HashMap<String, Arc<Gate>>>>,
    }
    #[async_trait]
    impl ProxyHttp for DelegatingProxy {
        type CTX = EdgionHttpContext;
        fn new_ctx(&self) -> Self::CTX {
            self.inner.new_ctx()
        }
        fn persist_connection_context(
            &self,
            _session: &Session,
            ctx: &Self::CTX,
        ) -> Option<Box<dyn std::any::Any + Send + Sync>> {
            self.inner.persist_connection_context(_session, ctx)
        }
        fn on_connection_reuse(
            &self,
            _session: &mut Session,
            ctx: &mut Self::CTX,
            _previous: Box<dyn std::any::Any + Send + Sync>,
        ) {
            self.inner.on_connection_reuse(_session, ctx, _previous)
        }
        async fn upstream_peer(
            &self,
            session: &mut Session,
            ctx: &mut Self::CTX,
        ) -> pingora_core::Result<Box<HttpPeer>> {
            let mut peer = self.inner.upstream_peer(session, ctx).await?;
            // This proof covers downstream H1/H2 to a controlled H1 origin only.
            peer.options.set_http_version(1, 1);
            Ok(peer)
        }
        fn init_downstream_modules(&self, modules: &mut HttpModules) {
            self.inner.init_downstream_modules(modules)
        }
        async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<bool>
        where
            Self::CTX: Send + Sync,
        {
            let handled = self.inner.request_filter(session, ctx).await?;
            if !handled {
                let id = session
                    .req_header()
                    .headers
                    .get("x-lifecycle-id")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                let gate = self.gates.lock().get(id).cloned();
                if let Some(gate) = gate {
                    let trailer = id.starts_with("cancel-trailer-");
                    let mut adapter = PingoraSessionAdapter::new(session, ctx);
                    if trailer {
                        assert!(adapter.install_upstream_response_trailer(Box::new(TrailerDelay(gate))));
                    } else {
                        assert!(
                            adapter.install_attached_response_processor(Box::new(Delay { gate, first: true })),
                            "test delay must install before response freeze"
                        );
                    }
                }
            }
            Ok(handled)
        }
        async fn early_request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<()>
        where
            Self::CTX: Send + Sync,
        {
            self.inner.early_request_filter(session, ctx).await
        }
        async fn request_body_filter_action(
            &self,
            session: &mut Session,
            body: &mut Option<bytes::Bytes>,
            event: RequestBodyEvent,
            ctx: &mut Self::CTX,
        ) -> pingora_core::Result<pingora_proxy::RequestBodyAction>
        where
            Self::CTX: Send + Sync,
        {
            self.inner.request_body_filter_action(session, body, event, ctx).await
        }
        fn request_relay_plan(&self, _session: &Session, ctx: &Self::CTX) -> pingora_proxy::RequestRelayPlan {
            self.inner.request_relay_plan(_session, ctx)
        }
        fn response_head_may_hold(&self, _session: &Session, ctx: &Self::CTX) -> bool {
            self.inner.response_head_may_hold(_session, ctx)
        }
        async fn upstream_request_filter(
            &self,
            session: &mut Session,
            upstream_request: &mut pingora_http::RequestHeader,
            ctx: &mut Self::CTX,
        ) -> pingora_core::Result<()>
        where
            Self::CTX: Send + Sync,
        {
            self.inner.upstream_request_filter(session, upstream_request, ctx).await
        }
        async fn upstream_response_filter(
            &self,
            session: &mut Session,
            upstream_response: &mut ResponseHeader,
            ctx: &mut Self::CTX,
        ) -> pingora_core::Result<()>
        where
            Self::CTX: Send + Sync,
        {
            self.inner
                .upstream_response_filter(session, upstream_response, ctx)
                .await
        }
        async fn upstream_response_header_filter_event(
            &self,
            session: &mut Session,
            upstream_response: &mut ResponseHeader,
            end_of_stream: bool,
            ctx: &mut Self::CTX,
        ) -> pingora_core::Result<()>
        where
            Self::CTX: Send + Sync,
        {
            self.inner
                .upstream_response_header_filter_event(session, upstream_response, end_of_stream, ctx)
                .await
        }
        async fn response_filter(
            &self,
            session: &mut Session,
            upstream_response: &mut ResponseHeader,
            ctx: &mut Self::CTX,
        ) -> pingora_core::Result<()>
        where
            Self::CTX: Send + Sync,
        {
            self.inner.response_filter(session, upstream_response, ctx).await
        }
        fn response_head_commit_plan(
            &self,
            session: &Session,
            source: ResponseHeadSource,
            response: &ResponseHeader,
            ctx: &Self::CTX,
        ) -> pingora_core::Result<ResponseHeadCommitPlan> {
            self.inner.response_head_commit_plan(session, source, response, ctx)
        }
        fn response_head_will_commit(
            &self,
            session: &Session,
            chosen_header: &ResponseHeader,
            ctx: &mut Self::CTX,
        ) -> pingora_core::Result<()> {
            self.inner.response_head_will_commit(session, chosen_header, ctx)
        }
        fn response_head_hold_boundary(
            &self,
            _session: &Session,
            boundary: ResponseHeadBoundary,
            ctx: &mut Self::CTX,
        ) -> ResponseHeadBoundaryAction {
            self.inner.response_head_hold_boundary(_session, boundary, ctx)
        }
        fn response_head_hold_outcome(
            &self,
            _session: &Session,
            outcome: ResponseHeadOutcome,
            usage: ResponseHeadUsage,
            ctx: &mut Self::CTX,
        ) {
            self.inner.response_head_hold_outcome(_session, outcome, usage, ctx)
        }
        async fn upstream_response_body_filter(
            &self,
            session: &mut Session,
            body: &mut Option<bytes::Bytes>,
            end_of_stream: bool,
            sink: &mut ResponseBodySink,
            ctx: &mut Self::CTX,
        ) -> pingora_core::Result<Option<std::time::Duration>>
        where
            Self::CTX: Send + Sync,
        {
            self.inner
                .upstream_response_body_filter(session, body, end_of_stream, sink, ctx)
                .await
        }
        async fn upstream_response_body_filter_event(
            &self,
            session: &mut Session,
            body: &mut Option<bytes::Bytes>,
            event: UpstreamResponseBodyEvent,
            sink: &mut ResponseBodySink,
            ctx: &mut Self::CTX,
        ) -> pingora_core::Result<Option<std::time::Duration>>
        where
            Self::CTX: Send + Sync,
        {
            self.inner
                .upstream_response_body_filter_event(session, body, event, sink, ctx)
                .await
        }
        async fn upstream_response_trailer_filter(
            &self,
            session: &mut Session,
            trailers: &mut http::HeaderMap,
            ctx: &mut Self::CTX,
        ) -> pingora_core::Result<()>
        where
            Self::CTX: Send + Sync,
        {
            self.inner
                .upstream_response_trailer_filter(session, trailers, ctx)
                .await
        }
        async fn logging(&self, session: &mut Session, e: Option<&Error>, ctx: &mut Self::CTX)
        where
            Self::CTX: Send + Sync,
        {
            self.inner.logging(session, e, ctx).await
        }
        fn error_while_proxy(
            &self,
            peer: &HttpPeer,
            session: &mut Session,
            e: Box<Error>,
            ctx: &mut Self::CTX,
            client_reused: bool,
        ) -> Box<Error> {
            self.inner.error_while_proxy(peer, session, e, ctx, client_reused)
        }
        fn fail_to_connect(
            &self,
            session: &mut Session,
            peer: &HttpPeer,
            ctx: &mut Self::CTX,
            e: Box<Error>,
        ) -> Box<Error> {
            self.inner.fail_to_connect(session, peer, ctx, e)
        }
        async fn fail_to_proxy(&self, session: &mut Session, e: &Error, ctx: &mut Self::CTX) -> FailToProxy
        where
            Self::CTX: Send + Sync,
        {
            self.inner.fail_to_proxy(session, e, ctx).await
        }
        fn suppress_error_log(&self, _session: &Session, ctx: &Self::CTX, _error: &Error) -> bool {
            self.inner.suppress_error_log(_session, ctx, _error)
        }
        fn request_summary(&self, session: &Session, ctx: &Self::CTX) -> String {
            self.inner.request_summary(session, ctx)
        }
        async fn connected_to_upstream(
            &self,
            session: &mut Session,
            reused: bool,
            peer: &HttpPeer,
            #[cfg(unix)] fd: std::os::unix::io::RawFd,
            #[cfg(windows)] sock: std::os::windows::io::RawSocket,
            digest: Option<&Digest>,
            ctx: &mut Self::CTX,
        ) -> pingora_core::Result<()>
        where
            Self::CTX: Send + Sync,
        {
            self.inner
                .connected_to_upstream(
                    session,
                    reused,
                    peer,
                    #[cfg(unix)]
                    fd,
                    #[cfg(windows)]
                    sock,
                    digest,
                    ctx,
                )
                .await
        }
    }
}
use proxy::DelegatingProxy;

const WAIT: Duration = Duration::from_secs(8);
const CANCEL_WAIT: Duration = Duration::from_secs(2);
const DELAY_DEADLINE: Duration = Duration::from_secs(15);
const CHUNK_BYTES: usize = 16 * 1024;
const MAX_CONNECTIONS: usize = 32;
const MAX_READ_AHEAD: usize = 32 * 1024 * 1024;
const FRAME: &[u8] = b"data: 0123456789abcdef01234567\n\n";
// Each matrix publishes process-global configuration and admission fixtures.
static MATRIX_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Default)]
struct Gate {
    entered: AtomicUsize,
    completed: AtomicUsize,
    cancelled: AtomicUsize,
    released_inflight: AtomicUsize,
    max_chunk: AtomicUsize,
    open: AtomicBool,
    wake: Notify,
}
impl Gate {
    fn release(&self) {
        self.open.store(true, Ordering::Release);
        self.wake.notify_waiters();
    }
    async fn hold(self: &Arc<Self>) {
        let mut guard = Inflight {
            gate: self.clone(),
            completed: false,
        };
        self.entered.fetch_add(1, Ordering::AcqRel);
        tokio::time::timeout(DELAY_DEADLINE, async {
            loop {
                let wake = self.wake.notified();
                if self.open.load(Ordering::Acquire) {
                    break;
                }
                wake.await;
            }
        })
        .await
        .expect("test delay deadline exceeded");
        guard.completed = true;
        self.completed.fetch_add(1, Ordering::AcqRel);
    }
}
struct Inflight {
    gate: Arc<Gate>,
    completed: bool,
}
impl Drop for Inflight {
    fn drop(&mut self) {
        if !self.completed {
            self.gate.cancelled.fetch_add(1, Ordering::AcqRel);
        }
    }
}
struct Delay {
    gate: Arc<Gate>,
    first: bool,
}
#[async_trait]
impl StreamingResponseProcessor for Delay {
    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        _: bool,
        _: ResponseStreamKind,
        _: &mut ResponseBodySink,
        _: &mut dyn edgion_gateway::plugins::runtime::traits::ResponseStreamSession,
    ) {
        if !self.first || chunk.as_ref().is_none_or(Bytes::is_empty) {
            return;
        }
        self.first = false;
        self.gate
            .max_chunk
            .fetch_max(chunk.as_ref().map_or(0, Bytes::len), Ordering::AcqRel);
        self.gate.hold().await;
    }
    fn release_inflight(&mut self) {
        self.gate.released_inflight.fetch_add(1, Ordering::AcqRel);
    }
}
struct TrailerDelay(Arc<Gate>);
#[async_trait]
impl edgion_gateway::plugins::runtime::UpstreamResponseTrailer for TrailerDelay {
    fn name(&self) -> &str {
        "TrailerDelay"
    }
    async fn run_upstream_response_trailer(
        &mut self,
        trailers: Option<&mut http::HeaderMap>,
        _: &mut dyn edgion_gateway::plugins::runtime::ResponseTrailerSession,
        _: &mut edgion_gateway::plugins::runtime::PluginLog,
    ) -> edgion_gateway::plugins::runtime::UpstreamTrailerResult {
        assert!(trailers.is_some(), "must await an actual transport trailer");
        self.0.hold().await;
        edgion_gateway::plugins::runtime::UpstreamTrailerResult::Continue
    }
    fn release_inflight(&mut self) {
        self.0.released_inflight.fetch_add(1, Ordering::AcqRel);
    }
}
async fn until(label: &str, mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(WAIT, async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out: {label}"));
}
fn trace_id(id: &str) -> String {
    hex::encode(&Sha256::digest(id.as_bytes())[..16])
}
fn traceparent(id: &str) -> String {
    format!("00-{}-0000000000000001-01", trace_id(id))
}
fn stats() -> Value {
    serde_json::to_value(edgion_gateway::plugins::http::wasm::admission::stats()).expect("admission serialization")
}
fn active_guest_instances() -> u64 {
    let sampled = stats();
    sampled["instances"].as_u64().expect("instance count")
        - sampled["idle_instances"].as_u64().expect("idle instance count")
}
fn accepted() -> Value {
    json!({"type":"Accepted","status":"True","reason":"Accepted","message":"","lastTransitionTime":"2026-10-15T00:00:00Z"})
}
fn publish(port: u16, origin: u16) {
    let config: edgion_resources::EdgionGatewayConfig = serde_json::from_value(json!({
        "metadata":{"name":"wasm-wire"},"spec":{"securityProtect":{"allowLoopbackUpstream":true}}}))
    .expect("gateway config");
    edgion_gateway::config::edgion_gateway::create_edgion_gateway_config_handler()
        .full_set(&HashMap::from([("wasm-wire".into(), Arc::new(config))]));
    let class: GatewayClass = serde_json::from_value(json!({"metadata":{"name":"wasm-wire"},"spec":{
        "controllerName":"edgion.io/gateway-controller","parametersRef":{"group":"edgion.io","kind":"EdgionGatewayConfig","name":"wasm-wire"}}})).expect("gateway class");
    edgion_gateway::config::gateway_class::create_gateway_class_handler()
        .full_set(&HashMap::from([("wasm-wire".into(), Arc::new(class))]));
    let gateway: Gateway = serde_json::from_value(json!({"metadata":{"name":"wasm-wire","namespace":"default"},"spec":{
        "gatewayClassName":"wasm-wire","listeners":[{"name":"http","port":port,"protocol":"HTTP"}],
        "currentStatus":{"listeners":[{"name":"http","supportedKinds":[],"attachedRoutes":0,"conditions":[accepted()]}]}}})).expect("gateway");
    edgion_gateway::runtime::create_gateway_handler()
        .full_set(&HashMap::from([("default/wasm-wire".into(), Arc::new(gateway))]));
    let plugin: EdgionPlugins = serde_json::from_value(json!({"metadata":{"name":"wasm-wire","namespace":"default"},"spec":{
        "requestPlugins":[{"type":"Wasm","config":{"source":{"file":concat!(env!("CARGO_MANIFEST_DIR"),"/../edgion-tests/wasm/artifacts/lifecycle_demo.wasm")},
        "timeoutMs":500,"maxActiveRequests":4,"instancePoolSize":4,"response":{"headers":true,"body":true,"trailers":true}}}]}})).expect("lifecycle plugin");
    let wasm =
        wat::parse_str(include_str!("support/wasm_stream_transport/lifecycle_probe.wat")).expect("proof WAT compile");
    println!(
        "host_lifecycle_probe_sha256={} source_sha256={}; fixture=non-SDK-WAT",
        hex::encode(Sha256::digest(&wasm)),
        hex::encode(Sha256::digest(include_bytes!(
            "support/wasm_stream_transport/lifecycle_probe.wat"
        )))
    );
    let proof: EdgionPlugins = serde_json::from_value(json!({"metadata":{"name":"wasm-proof","namespace":"default"},"spec":{
        "requestPlugins":[{"type":"Wasm","config":{"source":{"inline":base64::engine::general_purpose::STANDARD.encode(wasm)},
        "timeoutMs":500,"maxActiveRequests":4,"instancePoolSize":4,"response":{"headers":true,"body":true,"trailers":true}}}]}})).expect("proof plugin");
    edgion_gateway::plugins::http::create_plugin_handler().full_set(&HashMap::from([
        ("default/wasm-wire".into(), Arc::new(plugin)),
        ("default/wasm-proof".into(), Arc::new(proof)),
    ]));
    let coordinator = edgion_gateway::backends::discovery::get_service_discovery_coordinator();
    let service = serde_json::from_value(json!({"metadata":{"name":"wasm-origin","namespace":"default"},"spec":{
        "ports":[{"port":origin,"targetPort":origin,"protocol":"TCP"}]}}))
    .expect("service");
    let slice = serde_json::from_value(json!({"metadata":{"name":"wasm-origin","namespace":"default","labels":{"kubernetes.io/service-name":"wasm-origin"}},
        "addressType":"IPv4","ports":[{"port":origin,"protocol":"TCP"}],"endpoints":[{"addresses":["127.0.0.1"],"conditions":{"ready":true}}]})).expect("endpoint slice");
    coordinator.replace_services(&HashMap::from([("default/wasm-origin".into(), Arc::new(service))]));
    coordinator.replace_endpoint_slices(&HashMap::from([("default/wasm-origin".into(), Arc::new(slice))]));
    let mut routes = HashMap::new();
    for name in ["wasm", "proof", "plain"] {
        let parent = json!({"name":"wasm-wire"});
        let mut route: HTTPRoute = serde_json::from_value(json!({"metadata":{"name":name,"namespace":"default"},"spec":{
            "parentRefs":[parent.clone()],"hostnames":[format!("{name}.example.com")],"resolvedListeners":[format!("0.0.0.0:{port}/HTTP")],
            "resolvedStatusController":"edgion.io/gateway-controller","currentStatus":{"parents":[{"parentRef":parent,"controllerName":"edgion.io/gateway-controller","conditions":[accepted()]}]},
            "rules":[{"backendRefs":[{"name":"wasm-origin","port":origin}]}]}})).expect("route");
        if name != "plain" {
            route.spec.rules.as_mut().expect("route rules")[0].filters = Some(vec![serde_json::from_value(json!({
                "type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":if name == "wasm" {"wasm-wire"} else {"wasm-proof"}}}))
            .expect("plugin filter")]);
        }
        route.pre_parse();
        routes.insert(format!("default/{name}"), Arc::new(route));
    }
    edgion_gateway::routes::http::create_route_manager_handler().full_set(&routes);
}
fn field<'a>(headers: &'a str, name: &str) -> &'a str {
    headers
        .lines()
        .skip(1)
        .find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.trim())
        })
        .unwrap_or("")
}
#[derive(Default)]
struct Progress {
    sent: AtomicUsize,
    stopped: AtomicBool,
    finish: AtomicBool,
    begin: AtomicBool,
    wake: Notify,
}
impl Progress {
    fn begin(&self) {
        self.begin.store(true, Ordering::Release);
        self.wake.notify_waiters();
    }
}
struct OriginDrop(Arc<Progress>);
impl Drop for OriginDrop {
    fn drop(&mut self) {
        self.0.stopped.store(true, Ordering::Release);
    }
}
struct Started {
    id: String,
    progress: Arc<Progress>,
    request_marker: String,
}
async fn origin_h1(mut socket: TcpStream, started: mpsc::Sender<Started>) {
    // The request is tiny and bounded; no origin response is collected.
    let mut request = Vec::with_capacity(4096);
    let end = loop {
        let mut buf = [0; 1024];
        let n = match socket.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        request.extend_from_slice(&buf[..n]);
        assert!(request.len() <= 16 * 1024, "request fixture cap");
        if let Some(i) = request.windows(4).position(|v| v == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let headers = String::from_utf8(request[..end].to_vec()).expect("request headers UTF8");
    assert!(
        headers.lines().next().expect("request line").ends_with("HTTP/1.1"),
        "origin protocol must be H1"
    );
    let length = field(&headers, "content-length")
        .parse::<usize>()
        .expect("request content-length");
    assert!(length <= 256);
    while request.len() < end + length {
        let mut buf = [0; 256];
        let n = match socket.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        request.extend_from_slice(&buf[..n]);
    }
    let id = field(&headers, "x-lifecycle-id").to_owned();
    assert_eq!(&request[end..end + length], id.as_bytes());
    let mode = field(&headers, "x-wire-mode");
    let progress = Arc::new(Progress::default());
    let _closed = OriginDrop(progress.clone());
    started
        .send(Started {
            id: id.clone(),
            progress: progress.clone(),
            request_marker: field(&headers, "x-context-request").into(),
        })
        .await
        .expect("origin event receiver");
    if mode == "short" {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
        let _ = socket.write_all(response).await;
        return;
    }
    if socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nTrailer: x-lifecycle-trailer\r\nConnection: close\r\n\r\n").await.is_err() { return; }
    // Deterministic post-header body start, independent of protocol batching.
    let mut probe = [0; 1];
    tokio::select! {
        _ = async { loop { let wake = progress.wake.notified(); if progress.begin.load(Ordering::Acquire) { break; } wake.await; } } => {},
        _ = socket.read(&mut probe) => return,
        _ = tokio::time::sleep(WAIT) => return,
    }
    let data: Vec<u8> = FRAME.iter().copied().cycle().take(CHUNK_BYTES).collect();
    let prefix = format!("{:x}\r\n", data.len());
    loop {
        if progress.finish.load(Ordering::Acquire) {
            let tail = format!("0\r\nx-lifecycle-trailer: {id}\r\n\r\n");
            let _ = socket.write_all(tail.as_bytes()).await;
            return;
        }
        let sent = tokio::select! {
            result = async {
                socket.write_all(prefix.as_bytes()).await?;
                socket.write_all(&data).await?;
                socket.write_all(b"\r\n").await?;
                socket.flush().await
            } => result.is_ok(),
            _ = tokio::time::sleep(Duration::from_secs(30)) => false,
        };
        if !sent {
            return;
        }
        // This is successful awaited wire progress, not enqueueing into a channel.
        progress.sent.fetch_add(CHUNK_BYTES, Ordering::AcqRel);
        // A bounded producer prevents a ready socket from monopolizing the runtime.
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(1)) => {},
            _ = socket.read(&mut probe) => return,
        }
    }
}
struct Capture(mpsc::Sender<Value>);
#[async_trait]
impl edgion_gateway::link_sys::DataSender<String> for Capture {
    async fn init(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
    fn healthy(&self) -> bool {
        !self.0.is_closed()
    }
    async fn send(&self, data: String) -> anyhow::Result<()> {
        assert!(data.len() <= 64 * 1024, "bounded log fixture");
        self.0.try_send(serde_json::from_str(&data)?)?;
        Ok(())
    }
    fn name(&self) -> &str {
        "wasm-stream-wire-capture"
    }
}
struct Harness {
    port: u16,
    started: mpsc::Receiver<Started>,
    logs: mpsc::Receiver<Value>,
    gates: Arc<Mutex<HashMap<String, Arc<Gate>>>>,
    seen: HashSet<String>,
    pending_logs: HashMap<String, Value>,
    query_seq: usize,
    shutdown: watch::Sender<bool>,
    tasks: JoinSet<()>,
}
impl Harness {
    async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("gateway listener");
        let port = listener.local_addr().expect("gateway address").port();
        let origin = TcpListener::bind("127.0.0.1:0").await.expect("origin listener");
        publish(port, origin.local_addr().expect("origin address").port());
        let (tx, logs) = mpsc::channel(128);
        let mut logger = edgion_gateway::observe::AccessLogger::new();
        logger.register(Box::new(Capture(tx)));
        let key = ListenerKey(format!("0.0.0.0:{port}/HTTP"));
        let inner = EdgionHttpProxy {
            listener_port: port,
            is_https_listener: false,
            listener_key: key.clone(),
            server_header_opts: Default::default(),
            access_logger: Arc::new(logger),
            egc: edgion_gateway::config::bind_gateway_config(Some("wasm-wire")),
            enable_http2: true,
            preflight_handler: edgion_gateway::routes::http::proxy_http::PreflightHandler,
            http_route_manager: edgion_gateway::routes::http::get_global_http_route_managers()
                .get_or_create_listener_manager(&key),
            grpc_route_manager: edgion_gateway::routes::grpc::get_global_grpc_route_managers()
                .get_or_create_listener_manager(&key),
        };
        let gates = Arc::new(Mutex::new(HashMap::new()));
        let mut proxy = pingora_proxy::http_proxy(
            &Arc::new(pingora_core::server::configuration::ServerConf::default()),
            DelegatingProxy {
                inner,
                gates: gates.clone(),
            },
        );
        let mut options = pingora_core::apps::HttpServerOptions::default();
        options.h2c = true;
        proxy.server_options = Some(options);
        let proxy = Arc::new(proxy);
        let (shutdown, stop) = watch::channel(false);
        let mut gateway_stop = stop.clone();
        let mut tasks = JoinSet::new();
        tasks.spawn(async move {
            let mut connections = JoinSet::new();
            loop { tokio::select! {
                _ = gateway_stop.changed() => break,
                Some(result) = connections.join_next(), if !connections.is_empty() => { result.expect("gateway task"); },
                result = listener.accept(), if connections.len() < MAX_CONNECTIONS => {
                    let (socket, _) = result.expect("gateway accept");
                    let proxy = proxy.clone(); let stop = gateway_stop.clone();
                    connections.spawn(async move { let _ = proxy.process_new(Box::new(l4::stream::Stream::from(socket)), &stop).await; });
                }
            }}
            connections.abort_all(); while connections.join_next().await.is_some() {}
        });
        let (start_tx, started) = mpsc::channel(32);
        let mut origin_stop = stop;
        tasks.spawn(async move {
            let mut connections = JoinSet::new();
            loop { tokio::select! {
                _ = origin_stop.changed() => break,
                Some(result) = connections.join_next(), if !connections.is_empty() => { result.expect("origin task"); },
                result = origin.accept(), if connections.len() < MAX_CONNECTIONS => {
                    let (socket, _) = result.expect("origin accept"); let started = start_tx.clone();
                    connections.spawn(async move { origin_h1(socket, started).await; });
                }
            }}
            connections.abort_all(); while connections.join_next().await.is_some() {}
        });
        Self {
            port,
            started,
            logs,
            gates,
            seen: HashSet::new(),
            pending_logs: HashMap::new(),
            query_seq: 0,
            shutdown,
            tasks,
        }
    }
    fn gate(&self, id: &str) -> Arc<Gate> {
        let gate = Arc::new(Gate::default());
        assert!(self.gates.lock().insert(id.into(), gate.clone()).is_none());
        gate
    }
    async fn event(&mut self, id: &str, wasm: bool) -> Arc<Progress> {
        let event = tokio::time::timeout(WAIT, self.started.recv())
            .await
            .unwrap_or_else(|_| panic!("missing origin {id}; log={:?}", self.logs.try_recv()))
            .expect("origin event");
        assert_eq!(event.id, id);
        if wasm {
            assert_eq!(event.request_marker, id, "guest request-body callback did not execute");
        }
        event.progress
    }
    async fn log(&mut self, id: &str, status: u16) -> Value {
        let key = trace_id(id);
        let log = if let Some(log) = self.pending_logs.remove(&key) {
            log
        } else {
            tokio::time::timeout(WAIT, async {
                loop {
                    let log = self.logs.recv().await.expect("terminal log");
                    let observed = log["request_info"]["trace_id"]
                        .as_str()
                        .expect("canonical trace_id")
                        .to_owned();
                    assert!(
                        self.seen.insert(observed.clone()),
                        "duplicate terminal access log: {log}"
                    );
                    assert!(
                        self.seen.len() <= 128 && self.pending_logs.len() <= 32,
                        "log record cap"
                    );
                    if observed == key {
                        break log;
                    }
                    assert!(self.pending_logs.insert(observed, log).is_none());
                }
            })
            .await
            .expect("terminal log deadline")
        };
        assert_eq!(log["request_info"]["status"], status, "{log}");
        let serialized = log.to_string();
        assert!(
            !serialized.contains("WasmExec") && !serialized.contains("WasmTrap"),
            "guest failed: {log}"
        );
        log
    }
    async fn proof(&mut self, target: &str) -> [u8; 36] {
        let id = format!("query-{}", self.query_seq);
        self.query_seq += 1;
        let mut client = Client::start(self.port, false, &id, &format!("probe:{target}"), true).await;
        assert_eq!(
            tokio::time::timeout(WAIT, client.head(Some(&id)))
                .await
                .expect("probe header deadline"),
            200
        );
        self.event(&id, true).await;
        let proof = client.proof().expect("guest shared-data proof header");
        let mut size = 0;
        while let n @ 1.. = client.read_some().await {
            size += n;
            assert!(size <= 2);
        }
        assert_eq!(size, 2);
        client.close().await;
        self.log(&id, 200).await;
        proof
    }
    async fn close(mut self) {
        assert!(self.pending_logs.is_empty(), "unconsumed correlated terminal log");
        assert!(
            self.logs.try_recv().is_err(),
            "duplicate/unconsumed terminal access log"
        );
        let _ = self.shutdown.send(true);
        while let Some(result) = tokio::time::timeout(WAIT, self.tasks.join_next())
            .await
            .expect("harness shutdown")
        {
            result.expect("listener task");
        }
        assert!(
            self.logs.try_recv().is_err(),
            "late duplicate terminal log during completed teardown"
        );
        edgion_gateway::routes::http::create_route_manager_handler().full_set(&HashMap::new());
        edgion_gateway::plugins::http::create_plugin_handler().full_set(&HashMap::new());
        edgion_gateway::runtime::create_gateway_handler().full_set(&HashMap::new());
        edgion_gateway::config::gateway_class::create_gateway_class_handler().full_set(&HashMap::new());
        edgion_gateway::config::edgion_gateway::create_edgion_gateway_config_handler().full_set(&HashMap::new());
        let coordinator = edgion_gateway::backends::discovery::get_service_discovery_coordinator();
        coordinator.replace_services(&HashMap::new());
        coordinator.replace_endpoint_slices(&HashMap::new());
        until("guest allocation recovery after retirement", || {
            stats()["instances"] == 0 && stats()["guest_bytes"] == 0
        })
        .await;
        println!("retired admission={}", stats());
    }
}

// H1 parses one transport frame at a time; H2 retains its driver and SendStream.
// Neither client aggregates response payloads.
enum Client {
    H1 {
        socket: TcpStream,
        proof: Option<[u8; 36]>,
    },
    H2 {
        sender: h2::client::SendRequest<Bytes>,
        response: Option<h2::client::ResponseFuture>,
        body: Option<h2::RecvStream>,
        output: h2::SendStream<Bytes>,
        driver: tokio::task::JoinHandle<Result<(), h2::Error>>,
        proof: Option<[u8; 36]>,
    },
}
impl Drop for Client {
    fn drop(&mut self) {
        if let Self::H2 { driver, .. } = self {
            driver.abort();
        }
    }
}
impl Client {
    async fn start(port: u16, h2: bool, id: &str, mode: &str, wasm: bool) -> Self {
        let mut socket = TcpStream::connect(("127.0.0.1", port)).await.expect("client connect");
        let host = if wasm {
            if id.starts_with("sdk-") {
                "wasm.example.com"
            } else {
                "proof.example.com"
            }
        } else {
            "plain.example.com"
        };
        let target = mode.strip_prefix("probe:");
        let mode = if target.is_some() { "short" } else { mode };
        if h2 {
            let mut builder = h2::client::Builder::new();
            builder.initial_window_size(1024).initial_connection_window_size(1024);
            let (mut sender, connection) = builder.handshake(socket).await.expect("h2 handshake");
            let driver = tokio::spawn(connection);
            let request = http::Request::builder()
                .method("POST")
                .uri(format!("http://{host}/stream"))
                .header("content-length", id.len())
                .header("x-lifecycle-id", id)
                .header("x-wire-mode", mode)
                .header("traceparent", traceparent(id))
                .header("x-probe-target", target.unwrap_or(""))
                .body(())
                .expect("h2 request");
            let (response, mut output) = sender.send_request(request, false).expect("h2 send request");
            output
                .send_data(Bytes::copy_from_slice(id.as_bytes()), true)
                .expect("h2 request body");
            Self::H2 {
                sender,
                response: Some(response),
                body: None,
                output,
                driver,
                proof: None,
            }
        } else {
            let request = format!("POST /stream HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\nX-Lifecycle-Id: {id}\r\nX-Wire-Mode: {mode}\r\nConnection: close\r\nTraceparent: {}\r\nX-Probe-Target: {}\r\n\r\n{id}", id.len(),traceparent(id),target.unwrap_or(""));
            socket.write_all(request.as_bytes()).await.expect("h1 request");
            Self::H1 { socket, proof: None }
        }
    }
    async fn head(&mut self, id: Option<&str>) -> u16 {
        match self {
            Self::H1 { socket, proof } => {
                let mut header = Vec::new();
                let mut byte = [0];
                while !header.ends_with(b"\r\n\r\n") {
                    socket.read_exact(&mut byte).await.expect("h1 response header");
                    header.push(byte[0]);
                    assert!(header.len() <= 16 * 1024);
                }
                let header = String::from_utf8(header).expect("h1 response UTF8");
                let encoded = field(&header, "x-proof");
                if !encoded.is_empty() {
                    *proof = Some(
                        hex::decode(encoded)
                            .expect("proof hex")
                            .try_into()
                            .expect("proof record length"),
                    );
                }
                assert!(header.starts_with("HTTP/1.1"));
                if let Some(id) = id {
                    assert_eq!(
                        field(&header, "x-context-response"),
                        id,
                        "guest response header callback did not execute"
                    );
                }
                header
                    .split_whitespace()
                    .nth(1)
                    .expect("status")
                    .parse()
                    .expect("numeric status")
            }
            Self::H2 {
                response, body, proof, ..
            } => {
                let head = response.take().expect("h2 response once").await.expect("h2 response");
                assert_eq!(head.version(), http::Version::HTTP_2);
                if let Some(id) = id {
                    assert_eq!(head.headers()["x-context-response"], id);
                }
                if let Some(encoded) = head.headers().get("x-proof") {
                    *proof = Some(
                        hex::decode(encoded)
                            .expect("h2 proof hex")
                            .try_into()
                            .expect("h2 proof record length"),
                    );
                }
                let status = head.status().as_u16();
                *body = Some(head.into_body());
                status
            }
        }
    }
    async fn no_data(&mut self) {
        match self {
            Self::H1 { socket, .. } => {
                let mut byte = [0];
                assert!(
                    tokio::time::timeout(Duration::from_millis(100), socket.read(&mut byte))
                        .await
                        .is_err(),
                    "undecided H1 bytes escaped"
                );
            }
            Self::H2 { body, .. } => {
                assert!(
                    tokio::time::timeout(Duration::from_millis(100), body.as_mut().expect("h2 body").data())
                        .await
                        .is_err(),
                    "undecided H2 bytes escaped"
                );
            }
        }
    }
    async fn read_some(&mut self) -> usize {
        match self {
            Self::H1 { socket, .. } => {
                let mut buffer = [0; 32 * 1024];
                socket.read(&mut buffer).await.expect("h1 data")
            }
            Self::H2 { body, .. } => {
                let body = body.as_mut().expect("h2 response body");
                match body.data().await {
                    Some(chunk) => {
                        let chunk = chunk.expect("h2 data");
                        let n = chunk.len();
                        body.flow_control().release_capacity(n).expect("release h2 capacity");
                        n
                    }
                    None => 0,
                }
            }
        }
    }
    async fn finish(&mut self, trailer: Option<&str>) -> usize {
        match self {
            Self::H1 { socket, .. } => {
                // Read and validate chunked framing with a bounded single-chunk scratch.
                let mut total = 0usize;
                loop {
                    let mut line = Vec::with_capacity(64);
                    let mut byte = [0];
                    while !line.ends_with(b"\r\n") {
                        socket.read_exact(&mut byte).await.expect("chunk line");
                        line.push(byte[0]);
                        assert!(line.len() <= 1024);
                    }
                    let size = usize::from_str_radix(
                        std::str::from_utf8(&line[..line.len() - 2])
                            .expect("chunk size UTF8")
                            .split(';')
                            .next()
                            .expect("chunk size"),
                        16,
                    )
                    .expect("chunk size hex");
                    if size == 0 {
                        let mut trailers = Vec::new();
                        loop {
                            let mut line = Vec::new();
                            while !line.ends_with(b"\r\n") {
                                socket.read_exact(&mut byte).await.expect("trailer line");
                                line.push(byte[0]);
                                assert!(line.len() <= 1024);
                            }
                            if line == b"\r\n" {
                                break;
                            }
                            trailers.extend_from_slice(&line);
                            assert!(trailers.len() <= 4096);
                        }
                        if let Some(id) = trailer {
                            assert!(String::from_utf8_lossy(&trailers).contains(&format!("x-lifecycle-trailer: {id}")));
                        }
                        return total;
                    }
                    assert!(size <= 64 * 1024, "single wire chunk cap");
                    let mut scratch = vec![0; size + 2];
                    socket.read_exact(&mut scratch).await.expect("chunk data");
                    assert_eq!(&scratch[size..], b"\r\n");
                    for (i, value) in scratch[..size].iter().enumerate() {
                        assert_eq!(
                            *value,
                            FRAME[(total + i) % CHUNK_BYTES % FRAME.len()],
                            "ordered original SSE bytes"
                        );
                    }
                    total += size;
                }
            }
            Self::H2 { body, .. } => {
                let body = body.as_mut().expect("h2 response body");
                let mut total = 0;
                while let Some(chunk) = body.data().await {
                    let chunk = chunk.expect("h2 body data");
                    for (i, value) in chunk.iter().enumerate() {
                        assert_eq!(
                            *value,
                            FRAME[(total + i) % CHUNK_BYTES % FRAME.len()],
                            "ordered original SSE bytes"
                        );
                    }
                    total += chunk.len();
                    body.flow_control()
                        .release_capacity(chunk.len())
                        .expect("h2 flow control");
                }
                if let Some(id) = trailer {
                    let trailers = body.trailers().await.expect("h2 trailers result").expect("h2 trailers");
                    assert_eq!(trailers["x-lifecycle-trailer"], id);
                }
                total
            }
        }
    }
    fn proof(&self) -> Option<[u8; 36]> {
        match self {
            Self::H1 { proof, .. } | Self::H2 { proof, .. } => *proof,
        }
    }
    async fn plain_same_h2_connection(&mut self) {
        let Self::H2 { sender, .. } = self else {
            return;
        };
        std::future::poll_fn(|cx| sender.poll_ready(cx))
            .await
            .expect("same h2 connection ready");
        let request = http::Request::builder()
            .method("POST")
            .uri("http://plain.example.com/stream")
            .header("content-length", 5)
            .header("x-lifecycle-id", "probe")
            .header("traceparent", traceparent("probe"))
            .header("x-wire-mode", "short")
            .body(())
            .expect("plain h2 probe");
        let (response, mut output) = sender.send_request(request, false).expect("same h2 request");
        output
            .send_data(Bytes::from_static(b"probe"), true)
            .expect("same h2 request body");
        let response = response.await.expect("same h2 response");
        assert_eq!(response.status(), 200);
        let mut body = response.into_body();
        let mut total = 0;
        while let Some(chunk) = body.data().await {
            let chunk = chunk.expect("same h2 data");
            total += chunk.len();
            body.flow_control()
                .release_capacity(chunk.len())
                .expect("same h2 capacity");
        }
        assert_eq!(total, 2);
    }
    fn reset(&mut self) {
        match self {
            Self::H1 { socket, .. } => {
                let _ = socket;
                panic!("drop H1 client to close its socket");
            }
            Self::H2 { output, .. } => output.send_reset(h2::Reason::CANCEL),
        }
    }
    async fn close(&mut self) {
        if let Self::H2 { driver, .. } = self {
            driver.abort();
            let _ = driver.await;
        }
    }
}
async fn plateau(progress: &Progress) -> usize {
    let start = Instant::now();
    let mut previous = progress.sent.load(Ordering::Acquire);
    let mut stable = 0;
    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let next = progress.sent.load(Ordering::Acquire);
        assert!(
            next <= MAX_READ_AHEAD,
            "origin exceeded documented 32 MiB read-ahead allowance"
        );
        assert!(
            !progress.stopped.load(Ordering::Acquire),
            "plateau requires live origin"
        );
        stable = if previous == next { stable + 1 } else { 0 };
        if stable >= 2 {
            println!("origin plateau_bytes={next} settle_ms={}", start.elapsed().as_millis());
            return next;
        }
        assert!(
            start.elapsed() < WAIT,
            "origin failed to reach a bounded read-ahead plateau"
        );
        previous = next;
    }
}

async fn short(harness: &mut Harness, h2: bool, id: &str, wasm: bool, status: u16) -> Duration {
    let start = Instant::now();
    let mut client = Client::start(harness.port, h2, id, "short", wasm).await;
    assert_eq!(
        tokio::time::timeout(WAIT, client.head((wasm && status == 200).then_some(id)))
            .await
            .expect("short response deadline"),
        status
    );
    if status == 200 {
        harness.event(id, wasm).await;
    }
    let mut bytes = 0;
    while let n @ 1.. = tokio::time::timeout(WAIT, client.read_some())
        .await
        .expect("short data deadline")
    {
        bytes += n;
        assert!(bytes <= 4096);
    }
    if status == 200 {
        assert_eq!(bytes, 2);
    }
    client.close().await;
    harness.log(id, status).await;
    start.elapsed()
}

fn proof_u32(record: &[u8; 36], offset: usize) -> u32 {
    u32::from_le_bytes(record[offset..offset + 4].try_into().expect("proof field"))
}
fn assert_proof(record: &[u8; 36], bytes: Option<usize>, terminal: bool) {
    let observed = u64::from_le_bytes(record[..8].try_into().expect("proof bytes"));
    if let Some(bytes) = bytes {
        assert_eq!(observed, bytes as u64, "guest byte observation");
    }
    assert!(proof_u32(record, 8) > 0, "guest body callback");
    assert_eq!(proof_u32(record, 12), 1, "guest response headers");
    assert_eq!(
        proof_u32(record, 16),
        u32::from(terminal),
        "guest actual trailer callback"
    );
    assert_eq!(proof_u32(record, 20), u32::from(terminal), "guest EOS");
    assert_eq!(proof_u32(record, 24), 1, "guest on_done exactly once");
    assert_eq!(proof_u32(record, 28), 1, "guest request body completed");
    assert_eq!(proof_u32(record, 32), 1, "guest on_delete exactly once");
}
async fn sdk_smoke(harness: &mut Harness, h2: bool) {
    let id = if h2 { "sdk-h2" } else { "sdk-h1" };
    let mut client = Client::start(harness.port, h2, id, "stream", true).await;
    let progress = harness.event(id, true).await;
    assert_eq!(
        tokio::time::timeout(WAIT, client.head(Some(id)))
            .await
            .expect("SDK header deadline"),
        200
    );
    progress.begin();
    until("SDK origin body", || progress.sent.load(Ordering::Acquire) > 0).await;
    progress.finish.store(true, Ordering::Release);
    let bytes = tokio::time::timeout(WAIT, client.finish(Some(id)))
        .await
        .expect("SDK stream finish");
    assert_eq!(bytes, progress.sent.load(Ordering::Acquire));
    client.close().await;
    harness.log(id, 200).await;
    println!(
        "SDK wire smoke protocol={} bytes={bytes}; detailed counters use separate WAT host proof",
        if h2 { "h2" } else { "h1" }
    );
}
async fn transport_and_capacity() {
    let mut harness = Harness::new().await;
    println!("scope=downstream-H1/H2-to-H1-origin chunk_bytes={CHUNK_BYTES} read_ahead_cap={MAX_READ_AHEAD} admission_before={}",stats());
    for h2 in [false, true] {
        sdk_smoke(&mut harness, h2).await;
        let id = if h2 { "delay-h2" } else { "delay-h1" };
        let gate = harness.gate(id);
        let mut client = Client::start(harness.port, h2, id, "stream", true).await;
        let progress = harness.event(id, true).await;
        assert_eq!(
            tokio::time::timeout(WAIT, client.head(Some(id)))
                .await
                .expect("stream header deadline"),
            200
        );
        progress.begin();
        until("processor entered", || gate.entered.load(Ordering::Acquire) == 1).await;
        client.no_data().await;
        let held = plateau(&progress).await;
        assert!(held > 0);
        assert!(gate.max_chunk.load(Ordering::Acquire) > 0);
        assert!(gate.max_chunk.load(Ordering::Acquire) <= 64 * 1024);
        assert_eq!(gate.completed.load(Ordering::Acquire), 0);
        progress.finish.store(true, Ordering::Release);
        gate.release();
        let received = tokio::time::timeout(WAIT, client.finish(Some(id)))
            .await
            .expect("released stream deadline");
        assert!(received >= held);
        assert_eq!(received, progress.sent.load(Ordering::Acquire));
        assert_eq!(gate.completed.load(Ordering::Acquire), 1);
        assert_eq!(gate.cancelled.load(Ordering::Acquire), 0);
        client.close().await;
        let log = harness.log(id, 200).await;
        assert_proof(&harness.proof(id).await, Some(received), true);
        println!(
            "delayed protocol={} bytes={received} held_chunk={} admission={} diagnostics={}",
            if h2 { "h2" } else { "h1" },
            gate.max_chunk.load(Ordering::Acquire),
            stats(),
            log.get("diagnostics").unwrap_or(&Value::Null)
        );

        let id = if h2 { "slow-h2" } else { "slow-h1" };
        let mut client = Client::start(harness.port, h2, id, "stream", true).await;
        let progress = harness.event(id, true).await;
        assert_eq!(
            tokio::time::timeout(WAIT, client.head(Some(id)))
                .await
                .expect("slow header deadline"),
            200
        );
        progress.begin();
        let paused = plateau(&progress).await;
        let mut received = 0usize;
        tokio::time::timeout(WAIT, async {
            while progress.sent.load(Ordering::Acquire) <= paused {
                let n = client.read_some().await;
                assert!(n > 0);
                received += n;
            }
        })
        .await
        .expect("origin must resume with downstream consumption");
        assert!(received > 0);
        println!(
            "slow protocol={} plateau={paused} resumed={} consumed={received}",
            if h2 { "h2" } else { "h1" },
            progress.sent.load(Ordering::Acquire)
        );
        client.close().await;
        drop(client);
        until("slow origin disconnect", || progress.stopped.load(Ordering::Acquire)).await;
        harness.log(id, 200).await;
        let interrupted = harness.proof(id).await;
        assert!(proof_u32(&interrupted, 8) > 0, "guest observed stream chunks");
        assert_eq!(proof_u32(&interrupted, 12), 1);
        assert_eq!(proof_u32(&interrupted, 28), 1);
        assert_eq!(proof_u32(&interrupted, 16), 0, "no fabricated trailer");
        assert_eq!(proof_u32(&interrupted, 20), 0, "no fabricated EOS");
        // Closure may win in the transport pump between callbacks (normal
        // finalization), or in the response watcher (abort without guest entry).
        let normal_cleanup = proof_u32(&interrupted, 24);
        assert!(normal_cleanup <= 1);
        assert_eq!(proof_u32(&interrupted, 32), normal_cleanup);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(harness.proof(id).await, interrupted, "late interrupted-stream callback");
    }
    capacity(&mut harness, false).await;
    capacity(&mut harness, true).await;
    harness.close().await;
}

async fn capacity(harness: &mut Harness, sdk: bool) {
    let prefix = if sdk { "sdk-capacity" } else { "wat-capacity" };
    let fixture = if sdk {
        "Rust-SDK-lifecycle"
    } else {
        "non-SDK-WAT-host-proof"
    };
    // Warm pools and the fixture's dedicated timer VM are included in admission.
    let baseline = stats();
    let mut streams = Vec::new();
    for index in 0..4 {
        let id = format!("{prefix}-{index}");
        let mut client = Client::start(harness.port, index % 2 == 1, &id, "stream", true).await;
        let progress = harness.event(&id, true).await;
        assert_eq!(
            tokio::time::timeout(WAIT, client.head(Some(&id)))
                .await
                .expect("capacity header deadline"),
            200
        );
        progress.begin();
        streams.push((id, client, progress));
    }
    let active = stats();
    assert!(active["instances"].as_u64().expect("instance count") >= 4);
    assert!(
        active["instances"].as_u64().expect("instances") - active["idle_instances"].as_u64().expect("idle instances")
            >= 4,
        "four retained request leases"
    );
    short(harness, false, &format!("{prefix}-overflow"), true, 500).await;
    let mut timings = Vec::with_capacity(20);
    for index in 0..20 {
        timings.push(
            short(harness, index % 2 == 1, &format!("{prefix}-plain-{index}"), false, 200)
                .await
                .as_micros(),
        );
    }
    timings.sort_unstable();
    println!("capacity fixture={fixture} active_limit=4 retained=4 fifth_status=500 plain_samples=20 plain_connect_to_terminal_p50_us={} plain_connect_to_terminal_p99_us={} before={} during={active}",timings[9],timings[19],baseline);
    for (id, mut client, progress) in streams {
        progress.finish.store(true, Ordering::Release);
        tokio::time::timeout(WAIT, client.finish(Some(&id)))
            .await
            .expect("capacity release deadline");
        client.close().await;
        drop(client);
        harness.log(&id, 200).await;
        if !sdk {
            assert_proof(
                &harness.proof(&id).await,
                Some(progress.sent.load(Ordering::Acquire)),
                true,
            );
        }
    }
    short(harness, false, &format!("{prefix}-recovery"), true, 200).await;
    let recovered = stats();
    assert!(recovered["idle_instances"].as_u64().expect("idle count") >= 1);
    println!("capacity fixture={fixture} recovered={recovered}; guest_bytes is charged guest allocation, not RSS");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_stream_backpressure_and_capacity() {
    let _fixture = MATRIX_LOCK.lock().await;
    tokio::time::timeout(Duration::from_secs(90), transport_and_capacity())
        .await
        .expect("transport matrix outer deadline");
}

async fn cancellation(trailer: bool) {
    let mut harness = Harness::new().await;
    short(&mut harness, false, "cancel-warmup", true, 200).await;
    short(&mut harness, false, "sdk-cancel-warmup", true, 200).await;
    // The SDK fixture creates exactly one dedicated timer VM after its first
    // 100 ms tick. It is resident, not idle and not a request admission leak.
    until("SDK timer VM established before admission baseline", || {
        stats()["preparations"] == 0 && active_guest_instances() == 1
    })
    .await;
    let mut outcomes = Vec::new();
    for h2 in [false, true] {
        let id = match (trailer, h2) {
            (false, false) => "cancel-h1",
            (false, true) => "cancel-h2",
            (true, false) => "cancel-trailer-h1",
            (true, true) => "cancel-trailer-h2",
        };
        let baseline_active = active_guest_instances();
        let gate = harness.gate(id);
        let mut client = Some(Client::start(harness.port, h2, id, "stream", true).await);
        let progress = harness.event(id, true).await;
        assert_eq!(
            tokio::time::timeout(WAIT, client.as_mut().expect("live client").head(Some(id)))
                .await
                .expect("cancel header deadline"),
            200
        );
        if trailer {
            progress.finish.store(true, Ordering::Release);
        }
        progress.begin();
        until("cancellation processor entered", || {
            gate.entered.load(Ordering::Acquire) == 1
        })
        .await;
        let start = Instant::now();
        if h2 {
            client.as_mut().expect("h2 client").reset();
        } else {
            drop(client.take());
        }
        let prompt = tokio::time::timeout(CANCEL_WAIT, async {
            while gate.cancelled.load(Ordering::Acquire) != 1
                || gate.released_inflight.load(Ordering::Acquire) != 1
                || !progress.stopped.load(Ordering::Acquire)
                || active_guest_instances() > baseline_active
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok();
        let elapsed = start.elapsed();
        assert_eq!(
            gate.completed.load(Ordering::Acquire),
            0,
            "measurement precedes normal completion"
        );
        println!("cancellation protocol={} prompt={prompt} observation_ms={} entered={} cancelled={} release_inflight={} origin_stopped={} admission={} (gate remains closed; deadline_ms={})",
            if h2 {"h2"} else {"h1"},elapsed.as_millis(),gate.entered.load(Ordering::Acquire),gate.cancelled.load(Ordering::Acquire),gate.released_inflight.load(Ordering::Acquire),progress.stopped.load(Ordering::Acquire),stats(),DELAY_DEADLINE.as_millis());
        if h2 {
            tokio::time::timeout(WAIT, client.as_mut().expect("h2 client").plain_same_h2_connection())
                .await
                .expect("same H2 connection plain probe deadline");
            harness.event("probe", false).await;
            harness.log("probe", 200).await;
        }
        let at_disconnect = harness.proof(id).await;
        println!(
            "cancellation guest_before_gate_release protocol={} record_hex={}",
            if h2 { "h2" } else { "h1" },
            hex::encode(at_disconnect)
        );
        if prompt {
            let mut expected = [0u8; 36];
            expected[12..16].copy_from_slice(&1u32.to_le_bytes());
            expected[28..32].copy_from_slice(&1u32.to_le_bytes());
            assert_eq!(
                at_disconnect, expected,
                "cancelled execution must not re-enter body, trailers, EOS, done or delete"
            );
            assert!(
                active_guest_instances() <= baseline_active,
                "guest admission still held"
            );
            harness.log(id, 200).await;
            short(&mut harness, false, &format!("{id}-closed-gate-recovery"), true, 200).await;
            assert!(!gate.open.load(Ordering::Acquire), "recovery must precede gate release");
        }
        // Cleanup happens only AFTER the strict prompt-cancellation measurement.
        // Releasing a failed case never changes its recorded result to success.
        gate.release();
        until("origin ends after cleanup release", || {
            progress.stopped.load(Ordering::Acquire)
        })
        .await;
        if !prompt {
            harness.log(id, 200).await;
        }
        if let Some(mut client) = client {
            client.close().await;
        }
        let callbacks = gate.completed.load(Ordering::Acquire);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            gate.completed.load(Ordering::Acquire),
            callbacks,
            "late processor callback"
        );
        let before = harness.proof(id).await;
        println!(
            "cancellation guest_after_cleanup protocol={} record_hex={}",
            if h2 { "h2" } else { "h1" },
            hex::encode(before)
        );
        if prompt {
            assert_eq!(gate.cancelled.load(Ordering::Acquire), 1);
            assert_eq!(gate.released_inflight.load(Ordering::Acquire), 1);
            assert_eq!(gate.completed.load(Ordering::Acquire), 0);
            assert_eq!(
                before, at_disconnect,
                "cancelled guest re-entry during terminal cleanup"
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            harness.proof(id).await,
            before,
            "no late guest callbacks after terminal"
        );
        short(&mut harness, false, &format!("{id}-recovery"), true, 200).await;
        outcomes.push((h2, prompt));
    }
    harness.close().await;
    assert!(
        outcomes.iter().all(|(_, prompt)| *prompt),
        "pending response processor was not cancelled within 2 seconds: {outcomes:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_pending_processor_disconnect_cancels_promptly() {
    let _fixture = MATRIX_LOCK.lock().await;
    tokio::time::timeout(Duration::from_secs(60), cancellation(false))
        .await
        .expect("cancellation matrix outer deadline");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_pending_trailer_disconnect_cancels_promptly() {
    let _fixture = MATRIX_LOCK.lock().await;
    tokio::time::timeout(Duration::from_secs(60), cancellation(true))
        .await
        .expect("trailer cancellation matrix outer deadline");
}
