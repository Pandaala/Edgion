//! Real Edgion unary conversion and Hold wire regressions.
//! Standalone exception: this target starts no Gateway/Controller binaries and
//! never touches the shared topology. It publishes processed fixtures directly.
//! Upstream H2 is selected ONLY by the delegating test wrapper; production AI peers use H1.
#![cfg(feature = "test-util")]

use async_trait::async_trait;
use bytes::Bytes;
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_gateway::{routes::http::proxy_http::EdgionHttpProxy, EdgionHttpContext};
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
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

struct DelegatingProxy {
    inner: EdgionHttpProxy,
    h2: Arc<AtomicBool>,
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
    async fn upstream_peer(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<Box<HttpPeer>> {
        let mut peer = self.inner.upstream_peer(session, ctx).await?;
        if self.h2.load(Ordering::Acquire)
            && ctx
                .selected_backend
                .as_ref()
                .is_some_and(edgion_gateway::backends::SelectedHttpBackend::is_ai)
        {
            peer.options.set_http_version(2, 2);
        }
        Ok(peer)
    }
    fn init_downstream_modules(&self, modules: &mut HttpModules) {
        self.inner.init_downstream_modules(modules)
    }
    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<bool>
    where
        Self::CTX: Send + Sync,
    {
        self.inner.request_filter(session, ctx).await
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

use edgion_resources::{EdgionBackend, EdgionPlugins, Gateway, GatewayClass, HTTPRoute, ListenerKey, ResourceMeta};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, watch},
    task::JoinSet,
};
const WAIT: Duration = Duration::from_secs(5);
const NO_HEAD: Duration = Duration::from_millis(25);
const DOCUMENT:&[u8]=br#"{"id":"msg_wire","type":"message","role":"assistant","model":"provider","content":[{"type":"text","text":"hello wire"}],"stop_reason":"end_turn","usage":{"input_tokens":7,"output_tokens":11}}"#;
struct Started {
    id: String,
    conn: u64,
    protocol: &'static str,
    release: oneshot::Sender<()>,
    closed: oneshot::Receiver<()>,
    eos: oneshot::Receiver<()>,
    progress: mpsc::UnboundedReceiver<()>,
}
struct OriginFinished(Option<oneshot::Sender<()>>);
impl Drop for OriginFinished {
    fn drop(&mut self) {
        if let Some(done) = self.0.take() {
            let _ = done.send(());
        }
    }
}
struct Capture(mpsc::UnboundedSender<Value>);
#[async_trait]
impl edgion_gateway::link_sys::DataSender<String> for Capture {
    async fn init(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
    fn healthy(&self) -> bool {
        !self.0.is_closed()
    }
    async fn send(&self, data: String) -> anyhow::Result<()> {
        self.0.send(serde_json::from_str(&data)?)?;
        Ok(())
    }
    fn name(&self) -> &str {
        "unary-wire-capture"
    }
}
fn accepted() -> Value {
    json!({"type":"Accepted","status":"True","reason":"Accepted","message":"","lastTransitionTime":"2026-10-06T00:00:00Z"})
}
fn publish(port: u16, origin: u16) {
    let config: edgion_resources::EdgionGatewayConfig = serde_json::from_value(
        json!({"metadata":{"name":"unary-wire"},"spec":{"securityProtect":{"allowLoopbackUpstream":true}}}),
    )
    .unwrap();
    edgion_gateway::config::edgion_gateway::create_edgion_gateway_config_handler()
        .full_set(&HashMap::from([("unary-wire".into(), Arc::new(config))]));
    let class:GatewayClass=serde_json::from_value(json!({"metadata":{"name":"unary-wire"},"spec":{"controllerName":"edgion.io/gateway-controller","parametersRef":{"group":"edgion.io","kind":"EdgionGatewayConfig","name":"unary-wire"}}})).unwrap();
    edgion_gateway::config::gateway_class::create_gateway_class_handler()
        .full_set(&HashMap::from([("unary-wire".into(), Arc::new(class))]));
    let gateway:Gateway=serde_json::from_value(json!({"metadata":{"name":"unary-wire","namespace":"default"},"spec":{"gatewayClassName":"unary-wire","listeners":[{"name":"http","port":port,"protocol":"HTTP"}],"currentStatus":{"listeners":[{"name":"http","supportedKinds":[],"attachedRoutes":0,"conditions":[accepted()]}]}}})).unwrap();
    edgion_gateway::runtime::create_gateway_handler()
        .full_set(&HashMap::from([("default/unary-wire".into(), Arc::new(gateway))]));
    let plugin:EdgionPlugins=serde_json::from_value(json!({"metadata":{"name":"wire-ai","namespace":"default"},"spec":{"requestPlugins":[{"type":"AiProxy","config":{"api":"openaiChat","usageObservation":"provider","conversion":"chatToAnthropic","backendSelection":{"type":"Weighted"}}}]}})).unwrap();
    let native_plugin: EdgionPlugins = serde_json::from_value(json!({"metadata":{"name":"wire-native","namespace":"default"},"spec":{"requestPlugins":[{"type":"AiProxy","config":{"api":"openaiChat"}}]}})).unwrap();
    let off_conversion: EdgionPlugins = serde_json::from_value(json!({"metadata":{"name":"wire-conversion-off","namespace":"default"},"spec":{"requestPlugins":[{"type":"AiProxy","config":{"api":"openaiChat","conversion":"chatToAnthropic"}}]}})).unwrap();
    let bandwidth: EdgionPlugins = serde_json::from_value(json!({"metadata":{"name":"wire-bandwidth","namespace":"default"},"spec":{"upstreamResponseBodyFilterPlugins":[{"type":"BandwidthLimit","config":{"rate":"1024"}}]}})).unwrap();
    edgion_gateway::plugins::http::create_plugin_handler().full_set(&HashMap::from([
        ("default/wire-ai".into(), Arc::new(plugin)),
        ("default/wire-native".into(), Arc::new(native_plugin)),
        ("default/wire-conversion-off".into(), Arc::new(off_conversion)),
        ("default/wire-bandwidth".into(), Arc::new(bandwidth)),
    ]));
    let mut backends = HashMap::new();
    for (name, provider) in [("anthropic", "Anthropic"), ("native", "OpenAICompatible")] {
        let mut backend:EdgionBackend=serde_json::from_value(json!({"metadata":{"name":name,"namespace":"default"},"spec":{"ai":{"provider":provider,"endpoint":format!("http://127.0.0.1:{origin}"),"allowInsecureHttp":true,"credentialPool":{"credentials":[{"name":"primary","secretRef":{"name":"fixture"}}]},"models":[{"name":"claude-fixture"}],"defaults":{"streaming":true,"maxOutputTokens":64}}}})).unwrap();
        backend.spec.ai.credential_pool.credentials[0].credential.secret = Some(k8s_openapi::api::core::v1::Secret {
            data: Some(BTreeMap::from([(
                "api-key".into(),
                k8s_openapi::ByteString(b"fixture-only".to_vec()),
            )])),
            ..Default::default()
        });
        backends.insert(format!("default/{name}"), Arc::new(backend));
    }
    edgion_gateway::ai::create_edgion_backend_handler().full_set(&backends);
    let mut routes = HashMap::new();
    for (name, backend_name) in [
        ("anthropic", "anthropic"),
        ("anthropic-off", "anthropic"),
        ("native", "native"),
        ("anthropic-timeout", "anthropic"),
        ("anthropic-bandwidth", "anthropic"),
    ] {
        let parent = json!({"name":"unary-wire"});
        let mut route:HTTPRoute=serde_json::from_value(json!({"metadata":{"name":name,"namespace":"default"},"spec":{"parentRefs":[parent.clone()],"hostnames":[format!("{name}.example.com")],"resolvedListeners":[format!("0.0.0.0:{port}/HTTP")],"resolvedStatusController":"edgion.io/gateway-controller","currentStatus":{"parents":[{"parentRef":parent,"controllerName":"edgion.io/gateway-controller","conditions":[accepted()]}]},"rules":[{"resolvedTerminalRouteUid":format!("HTTPRoute/default/{name}"),"resolvedTerminalRuleIdentity":"index:0","resolvedAiAdmission":{"backends":[{"kind":"edgionBackend","provider":"OpenAICompatible","hasCredentialTpm":false,"specValid":true,"canonicalModels":["claude-fixture"]}]},"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"wire-ai"}}],"backendRefs":[{"group":"edgion.io","kind":"EdgionBackend","name":backend_name}]}]}})).unwrap();
        let rule = &mut route.spec.rules.as_mut().unwrap()[0];
        if let Some(edgion_resources::resources::http_route_ai_admission::ResolvedForwardingTarget::EdgionBackend {
            provider,
            ..
        }) = rule
            .resolved_ai_admission
            .as_mut()
            .and_then(|facts| facts.backends.first_mut())
        {
            *provider = if backend_name == "native" {
                edgion_resources::resources::edgion_backend::AiProvider::OpenAICompatible
            } else {
                edgion_resources::resources::edgion_backend::AiProvider::Anthropic
            };
        }
        if backend_name == "native" {
            rule.filters.as_mut().unwrap()[0].extension_ref.as_mut().unwrap().name = "wire-native".to_string();
        }
        if name == "anthropic-off" {
            rule.filters.as_mut().unwrap()[0].extension_ref.as_mut().unwrap().name = "wire-conversion-off".to_string();
        }
        if name == "anthropic-timeout" {
            rule.timeouts = Some(serde_json::from_value(json!({"request":"500ms"})).unwrap());
        }
        if name == "anthropic-bandwidth" {
            rule.filters.as_mut().unwrap().push(serde_json::from_value(json!({"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"wire-bandwidth"}})).unwrap());
        }
        route.pre_parse();
        routes.insert(format!("default/{name}"), Arc::new(route));
    }
    let coordinator = edgion_gateway::backends::discovery::get_service_discovery_coordinator();
    let service:k8s_openapi::api::core::v1::Service=serde_json::from_value(json!({"metadata":{"name":"ordinary","namespace":"default"},"spec":{"ports":[{"port":origin,"targetPort":origin,"protocol":"TCP"}]}})).unwrap();
    let slice:k8s_openapi::api::discovery::v1::EndpointSlice=serde_json::from_value(json!({"metadata":{"name":"ordinary-slice","namespace":"default","labels":{"kubernetes.io/service-name":"ordinary"}},"addressType":"IPv4","ports":[{"port":origin,"protocol":"TCP"}],"endpoints":[{"addresses":["127.0.0.1"],"conditions":{"ready":true}}]})).unwrap();
    coordinator.replace_services(&HashMap::from([("default/ordinary".into(), Arc::new(service))]));
    coordinator.replace_endpoint_slices(&HashMap::from([("default/ordinary-slice".into(), Arc::new(slice))]));
    let parent = json!({"name":"unary-wire"});
    let mut ordinary:HTTPRoute=serde_json::from_value(json!({"metadata":{"name":"ordinary","namespace":"default"},"spec":{"parentRefs":[parent.clone()],"hostnames":["ordinary.example.com"],"resolvedListeners":[format!("0.0.0.0:{port}/HTTP")],"resolvedStatusController":"edgion.io/gateway-controller","currentStatus":{"parents":[{"parentRef":parent,"controllerName":"edgion.io/gateway-controller","conditions":[accepted()]}]},"rules":[{"backendRefs":[{"name":"ordinary","port":origin}]}]}})).unwrap();
    ordinary.pre_parse();
    routes.insert("default/ordinary".into(), Arc::new(ordinary));
    edgion_gateway::routes::http::create_route_manager_handler().full_set(&routes);
}
fn fixture_document(mode: &str) -> Bytes {
    let mut doc: Value = serde_json::from_slice(DOCUMENT).unwrap();
    match mode {
        "input-overflow" => doc["content"][0]["text"] = json!("a".repeat(1024 * 1024 + 1)),
        "output-overflow" => {
            doc["content"] =
                json!([{"type":"tool_use","id":"tool_wire","name":"inspect","input":{"value":"\\".repeat(300_000)}}]);
            doc["stop_reason"] = json!("tool_use");
        }
        "slow" => doc["content"][0]["text"] = json!("a".repeat(800_000)),
        "interleaved-fragment" => {
            doc["content"] = json!([{"type":"text","text":"hello "},{"type":"tool_use","id":"tool_wire","name":"inspect","input":{"value":"hello\\world","enabled":true}},{"type":"text","text":"wire"}]);
            doc["stop_reason"] = json!("tool_use");
            doc["usage"] = json!({"input_tokens":3,"output_tokens":11,"cache_read_input_tokens":7,"cache_creation_input_tokens":5});
        }
        _ => {}
    }
    Bytes::from(serde_json::to_vec(&doc).unwrap())
}
fn fixture_body(mode: &str) -> Bytes {
    match mode {
        "native-sse" => Bytes::from_static(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"native early\"}}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":11}}\n\ndata: [DONE]\n\n"),
        "malformed" => Bytes::from_static(b"{\"provider_secret\":\"must-not-leak\"} broken"),
        "truncated" | "trickle" => Bytes::from_static(b"{\"content\":["),
        "native" | "ordinary" => Bytes::from_static(br#"{"id":"chat_wire","choices":[{"message":{"content":"native early"}}],"usage":{"prompt_tokens":7,"completion_tokens":11}}"#),
        "sse" => Bytes::from([
            format!("event: message_start\ndata: {}\n\n",json!({"type":"message_start","message":{"type":"message","id":"msg_wire","model":"provider","role":"assistant","content":[],"usage":{"input_tokens":7,"output_tokens":0}}})),
            format!("event: content_block_start\ndata: {}\n\n",json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
            format!("event: content_block_delta\ndata: {}\n\n",json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"early text"}})),
            format!("event: content_block_stop\ndata: {}\n\n",json!({"type":"content_block_stop","index":0})),
        ].concat()),
        "fragment" | "interleaved-fragment" => { let doc=fixture_document(mode);doc.slice(..doc.len()/2) },
        "input-overflow" => fixture_document(mode).slice(..32768),
        _ => fixture_document(mode),
    }
}
fn fixture_tail(mode: &str) -> Option<Bytes> {
    if matches!(mode, "fragment" | "interleaved-fragment" | "input-overflow") {
        let doc = fixture_document(mode);
        let offset = if mode == "input-overflow" { 32768 } else { doc.len() / 2 };
        Some(doc.slice(offset..))
    } else {
        None
    }
}
fn field<'a>(headers: &'a str, name: &str) -> &'a str {
    headers
        .lines()
        .skip(1)
        .find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.trim())
        })
        .unwrap_or("")
}
async fn origin_h1(mut socket: TcpStream, conn: u64, started: mpsc::UnboundedSender<Started>) {
    loop {
        let mut request = Vec::new();
        let end = loop {
            let mut chunk = [0u8; 4096];
            match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => request.extend_from_slice(&chunk[..n]),
            };
            if let Some(i) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
        };
        let header = String::from_utf8_lossy(&request[..end]).into_owned();
        let length = field(&header, "content-length").parse::<usize>().unwrap_or(0);
        while request.len() < end + length {
            let mut chunk = [0u8; 4096];
            match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => request.extend_from_slice(&chunk[..n]),
            };
        }
        let id = field(&header, "x-wire-id").to_string();
        let mode = field(&header, "x-wire-mode").to_string();
        if matches!(mode.as_str(), "preheader-eof" | "preheader-timeout") {
            let (release, gate) = oneshot::channel();
            let (closed, finished) = oneshot::channel();
            let (_eos_tx, eos) = oneshot::channel();
            let (_progress_tx, progress) = mpsc::unbounded_channel();
            let _finished = OriginFinished(Some(closed));
            if started
                .send(Started {
                    id,
                    conn,
                    protocol: "h1",
                    release,
                    closed: finished,
                    eos,
                    progress,
                })
                .is_err()
            {
                return;
            }
            let mut probe = [0];
            tokio::select! { _=gate=>{}, _=socket.read(&mut probe)=>{} }
            return;
        }
        let body = fixture_body(&mode);
        let media = if mode == "wrong-media" {
            "text/plain"
        } else if matches!(mode.as_str(), "sse" | "native-sse") {
            "text/event-stream"
        } else {
            "application/json"
        };
        let encoding = if matches!(mode.as_str(), "gzip" | "off-gzip") {
            "Content-Encoding: gzip\r\n"
        } else {
            ""
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {media}\r\n{encoding}Transfer-Encoding: chunked\r\n\r\n{:x}\r\n",
            body.len()
        );
        if socket.write_all(response.as_bytes()).await.is_err()
            || socket.write_all(&body).await.is_err()
            || socket.write_all(b"\r\n").await.is_err()
            || socket.flush().await.is_err()
        {
            return;
        }
        let (tx, rx) = oneshot::channel();
        let (closed, finished) = oneshot::channel();
        let (eos_tx, eos) = oneshot::channel();
        let (progress_tx, progress) = mpsc::unbounded_channel();
        let _finished = OriginFinished(Some(closed));
        if started
            .send(Started {
                id,
                conn,
                protocol: "h1",
                release: tx,
                closed: finished,
                eos,
                progress,
            })
            .is_err()
        {
            return;
        }
        let mut probe = [0];
        tokio::select! {result=rx=>{if result.is_err(){return;}},_=socket.read(&mut probe)=>return}
        if mode == "reset" {
            return;
        }
        if mode == "trickle" {
            let mut interval = tokio::time::interval(Duration::from_millis(50));
            loop {
                tokio::select! {
                    _=interval.tick()=> { if socket.write_all(b"1\r\n \r\n").await.is_err() || socket.flush().await.is_err() { return; } let _=progress_tx.send(()); },
                    _=socket.read(&mut probe)=>return,
                }
            }
        }
        if let Some(tail) = fixture_tail(&mode) {
            let frame = format!("{:x}\r\n", tail.len());
            if socket.write_all(frame.as_bytes()).await.is_err()
                || socket.write_all(tail.as_ref()).await.is_err()
                || socket.write_all(b"\r\n").await.is_err()
            {
                return;
            }
        }
        if mode == "sse" {
            let tail=b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":11}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
            let part = format!("{:x}\r\n", tail.len());
            if socket.write_all(part.as_bytes()).await.is_err()
                || socket.write_all(tail.as_ref()).await.is_err()
                || socket.write_all(b"\r\n").await.is_err()
            {
                return;
            }
        }
        if mode == "trailer-overflow" {
            let mut terminal = String::from("0\r\n");
            for i in 0..65 {
                terminal.push_str(&format!("X-Field-{i}: value\r\n"));
            }
            terminal.push_str("\r\n");
            let _ = socket.write_all(terminal.as_bytes()).await;
            let _ = socket.flush().await;
            return;
        }
        if mode == "trailer-cumulative" {
            let terminal = format!("0\r\nX-Result: {}\r\n\r\n", "a".repeat(8116));
            let _ = socket.write_all(terminal.as_bytes()).await;
            let _ = socket.flush().await;
            return;
        }
        let terminal = if matches!(mode.as_str(), "trailers" | "trailers-bandwidth") {
            b"0\r\nX-Result: complete\r\n\r\n".as_slice()
        } else {
            b"0\r\n\r\n".as_slice()
        };
        if socket.write_all(terminal).await.is_err() || socket.flush().await.is_err() {
            return;
        }
        let _ = eos_tx.send(());
    }
}
async fn flush_origin(flush: &mpsc::Sender<oneshot::Sender<()>>) -> bool {
    let (ack, done) = oneshot::channel();
    flush.send(ack).await.is_ok() && done.await.is_ok()
}
async fn origin_h2(socket: TcpStream, conn: u64, started: mpsc::UnboundedSender<Started>) {
    let Ok(mut connection) = h2::server::handshake(socket).await else {
        return;
    };
    let (flush_tx, mut flush_rx) = mpsc::channel::<oneshot::Sender<()>>(16);
    let mut streams = JoinSet::new();
    loop {
        type Accepted = Option<Result<(http::Request<h2::RecvStream>, h2::server::SendResponse<Bytes>), h2::Error>>;
        enum Step {
            Request(Box<Accepted>),
            Flush(oneshot::Sender<()>),
        }
        let step = std::future::poll_fn(|cx| {
            if let std::task::Poll::Ready(result) = connection.poll_accept(cx) {
                return std::task::Poll::Ready(Step::Request(Box::new(result)));
            }
            if let std::task::Poll::Ready(Some(ack)) = flush_rx.poll_recv(cx) {
                return std::task::Poll::Ready(Step::Flush(ack));
            }
            std::task::Poll::Pending
        })
        .await;
        match step {
            Step::Flush(ack) => {
                let _ = ack.send(());
            }
            Step::Request(request) => {
                let Some(Ok((request, mut response))) = *request else {
                    break;
                };
                let started = started.clone();
                let flush = flush_tx.clone();
                streams.spawn(async move {
                    let id=request.headers()["x-wire-id"].to_str().unwrap().to_string();
                    let mode=request.headers()["x-wire-mode"].to_str().unwrap().to_string();
                    let mut input=request.into_body();
                    while let Some(Ok(chunk))=input.data().await { let _=input.flow_control().release_capacity(chunk.len()); }
                    if matches!(mode.as_str(), "preheader-eof" | "preheader-timeout") {
                        let (release, gate) = oneshot::channel();
                        let (closed, finished) = oneshot::channel();
                        let (_eos_tx, eos) = oneshot::channel();
                        let (_progress_tx, progress) = mpsc::unbounded_channel();
                        let _finished = OriginFinished(Some(closed));
                        if started.send(Started { id, conn, protocol: "h2", release, closed: finished, eos, progress }).is_err() { return; }
                        let _ = gate.await;
                        response.send_reset(h2::Reason::CANCEL);
                        return;
                    }
                    let media=if mode=="wrong-media" {"text/plain"} else if matches!(mode.as_str(),"sse"|"native-sse") {"text/event-stream"} else {"application/json"};
                    let mut head=http::Response::builder().status(200).header("content-type",media);
                    if matches!(mode.as_str(),"gzip"|"off-gzip") { head=head.header("content-encoding","gzip"); }
                    let Ok(mut output)=response.send_response(head.body(()).unwrap(),false) else { return; };
                    if output.send_data(fixture_body(&mode),false).is_err() {return;}
                    if !flush_origin(&flush).await {return;}
                    let (release,gate)=oneshot::channel(); let (closed,finished)=oneshot::channel();
                    let (eos_tx,eos)=oneshot::channel(); let (progress_tx,progress)=mpsc::unbounded_channel();
                    let _finished=OriginFinished(Some(closed));
                    if started.send(Started{id,conn,protocol:"h2",release,closed:finished,eos,progress}).is_err() {return;}
                    tokio::select! {result=gate=>{if result.is_err(){return;}},_=std::future::poll_fn(|cx|output.poll_reset(cx))=>return}
                    if mode=="reset" {output.send_reset(h2::Reason::INTERNAL_ERROR);return;}
                    if mode=="trickle" {
                        let mut interval=tokio::time::interval(Duration::from_millis(50));
                        loop { tokio::select! {
                            _=interval.tick()=> { if output.send_data(Bytes::from_static(b" "),false).is_err() || !flush_origin(&flush).await {return;} let _=progress_tx.send(()); },
                            _=std::future::poll_fn(|cx|output.poll_reset(cx))=>return,
                        } }
                    }
                    if let Some(tail)=fixture_tail(&mode) { if output.send_data(tail,false).is_err() {return;} }
                    if mode=="sse" { let _=output.send_data(Bytes::from_static(b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":11}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"),true); }
                    else if matches!(mode.as_str(),"trailers"|"trailers-bandwidth"|"trailer-cumulative"|"trailer-overflow") {
                        let mut trailers=http::HeaderMap::new();
                        if mode=="trailer-overflow" {for i in 0..65 {trailers.insert(http::HeaderName::from_bytes(format!("x-field-{i}").as_bytes()).unwrap(),http::HeaderValue::from_static("value"));}}
                        else {trailers.insert("x-result",http::HeaderValue::from_str(&if mode=="trailer-cumulative" {"a".repeat(8116)} else {"complete".into()}).unwrap());}
                        let _=output.send_trailers(trailers);
                    } else { let _=output.send_data(Bytes::new(),true); }
                    if flush_origin(&flush).await { let _=eos_tx.send(()); }
                });
            }
        }
    }
    streams.abort_all();
    while streams.join_next().await.is_some() {}
}

struct Harness {
    port: u16,
    origin: u16,
    h2: Arc<AtomicBool>,
    started: mpsc::UnboundedReceiver<Started>,
    logs: mpsc::UnboundedReceiver<Value>,
    shutdown: watch::Sender<bool>,
    tasks: JoinSet<()>,
}
impl Harness {
    async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_port = origin.local_addr().unwrap().port();
        publish(port, origin_port);
        let h2 = Arc::new(AtomicBool::new(false));
        let (log_tx, logs) = mpsc::unbounded_channel();
        let mut logger = edgion_gateway::observe::AccessLogger::new();
        logger.register(Box::new(Capture(log_tx)));
        let key = ListenerKey(format!("0.0.0.0:{port}/HTTP"));
        let inner = EdgionHttpProxy {
            listener_port: port,
            is_https_listener: false,
            listener_key: key.clone(),
            server_header_opts: Default::default(),
            access_logger: Arc::new(logger),
            egc: edgion_gateway::config::bind_gateway_config(Some("unary-wire")),
            enable_http2: true,
            preflight_handler: edgion_gateway::routes::http::proxy_http::PreflightHandler,
            http_route_manager: edgion_gateway::routes::http::get_global_http_route_managers()
                .get_or_create_listener_manager(&key),
            grpc_route_manager: edgion_gateway::routes::grpc::get_global_grpc_route_managers()
                .get_or_create_listener_manager(&key),
        };
        let mut proxy = pingora_proxy::http_proxy(
            &Arc::new(pingora_core::server::configuration::ServerConf::default()),
            DelegatingProxy { inner, h2: h2.clone() },
        );
        let mut options = pingora_core::apps::HttpServerOptions::default();
        options.h2c = true;
        proxy.server_options = Some(options);
        let proxy = Arc::new(proxy);
        let (shutdown, stop) = watch::channel(false);
        let (start_tx, started) = mpsc::unbounded_channel();
        let mut tasks = JoinSet::new();
        let mut gateway_stop = stop.clone();
        tasks.spawn(async move{let mut connections=JoinSet::new();loop{tokio::select!{_=gateway_stop.changed()=>break,result=listener.accept()=>{let(socket,_)=result.unwrap();let proxy=proxy.clone();let stop=gateway_stop.clone();connections.spawn(async move{let _=proxy.process_new(Box::new(l4::stream::Stream::from(socket)),&stop).await;});}}}connections.abort_all();while connections.join_next().await.is_some(){}});
        let mut origin_stop = stop;
        tasks.spawn(async move{let mut connections=JoinSet::new();let mut conn=0;loop{tokio::select!{_=origin_stop.changed()=>break,result=origin.accept()=>{let(socket,_)=result.unwrap();conn+=1;let started=start_tx.clone();connections.spawn(async move{let mut prefix=[0;24];let n=socket.peek(&mut prefix).await.unwrap_or(0);if n>0&&prefix[..n].starts_with(b"PRI * HTTP/2.0"){origin_h2(socket,conn,started).await;}else{origin_h1(socket,conn,started).await;}});}}}connections.abort_all();while connections.join_next().await.is_some(){}});
        Self {
            port,
            origin: origin_port,
            h2,
            started,
            logs,
            shutdown,
            tasks,
        }
    }
    async fn event(&mut self, id: &str) -> Started {
        let event = tokio::time::timeout(WAIT, self.started.recv())
            .await
            .unwrap_or_else(|_| panic!("provider did not receive {id}; terminal={:?}", self.logs.try_recv()))
            .unwrap();
        assert_eq!(event.id, id);
        event
    }
    async fn log(&mut self, status: u16) -> Value {
        let log = tokio::time::timeout(WAIT, self.logs.recv())
            .await
            .expect("missing terminal access log")
            .unwrap();
        assert_eq!(log["request_info"]["status"], status, "{log}");
        log
    }
    async fn close(mut self) {
        let _ = self.shutdown.send(true);
        while let Some(result) = tokio::time::timeout(WAIT, self.tasks.join_next()).await.unwrap() {
            result.unwrap();
        }
        edgion_gateway::routes::http::create_route_manager_handler().full_set(&HashMap::new());
        edgion_gateway::ai::create_edgion_backend_handler().full_set(&HashMap::new());
        edgion_gateway::plugins::http::create_plugin_handler().full_set(&HashMap::new());
        edgion_gateway::runtime::create_gateway_handler().full_set(&HashMap::new());
        edgion_gateway::config::gateway_class::create_gateway_class_handler().full_set(&HashMap::new());
        edgion_gateway::config::edgion_gateway::create_edgion_gateway_config_handler().full_set(&HashMap::new());
        let coordinator = edgion_gateway::backends::discovery::get_service_discovery_coordinator();
        coordinator.replace_services(&HashMap::new());
        coordinator.replace_endpoint_slices(&HashMap::new());
    }
}
enum Client {
    H1 {
        socket: TcpStream,
        prefix: Vec<u8>,
    },
    H2 {
        sender: h2::client::SendRequest<Bytes>,
        response: Option<h2::client::ResponseFuture>,
        body: Option<h2::RecvStream>,
        prefix: Vec<u8>,
        driver: tokio::task::JoinHandle<Result<(), h2::Error>>,
    },
}
impl Client {
    async fn start(port: u16, h2: bool, id: &str, mode: &str, native: bool) -> Self {
        let host = if mode.starts_with("off") {
            "anthropic-off.example.com"
        } else if mode == "preheader-timeout" {
            "anthropic-timeout.example.com"
        } else if mode == "trailers-bandwidth" {
            "anthropic-bandwidth.example.com"
        } else if mode == "ordinary" {
            "ordinary.example.com"
        } else if native {
            "native.example.com"
        } else {
            "anthropic.example.com"
        };
        let body = if matches!(mode, "sse" | "native-sse") {
            br#"{"model":"claude-fixture","messages":[{"role":"user","content":"hello"}],"stream":true}"#.as_slice()
        } else {
            br#"{"model":"claude-fixture","messages":[{"role":"user","content":"hello"}]}"#.as_slice()
        };
        let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        if h2 {
            let mut builder = h2::client::Builder::new();
            if mode == "slow" {
                builder.initial_window_size(1024).initial_connection_window_size(1024);
            }
            let (mut sender, connection) = builder.handshake(socket).await.unwrap();
            let driver = tokio::spawn(connection);
            let request = http::Request::builder()
                .method("POST")
                .uri(format!("http://{host}/v1/chat/completions"))
                .header("content-type", "application/json")
                .header("content-length", body.len())
                .header("x-wire-id", id)
                .header("x-wire-mode", mode)
                .body(())
                .unwrap();
            let (response, mut stream) = sender.send_request(request, false).unwrap();
            stream.send_data(Bytes::copy_from_slice(body), true).unwrap();
            Self::H2 {
                sender,
                response: Some(response),
                body: None,
                prefix: Vec::new(),
                driver,
            }
        } else {
            let request = format!("POST /v1/chat/completions HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nX-Wire-Id: {id}\r\nX-Wire-Mode: {mode}\r\n\r\n", body.len());
            socket.write_all(request.as_bytes()).await.unwrap();
            socket.write_all(body).await.unwrap();
            Self::H1 {
                socket,
                prefix: Vec::new(),
            }
        }
    }
    async fn no_head(&mut self) {
        match self {
            Self::H1 { socket, .. } => {
                let mut b = [0];
                assert!(
                    tokio::time::timeout(NO_HEAD, socket.read(&mut b)).await.is_err(),
                    "final head arrived before source terminal"
                );
            }
            Self::H2 { response, .. } => assert!(
                tokio::time::timeout(NO_HEAD, response.as_mut().unwrap()).await.is_err(),
                "H2 final head arrived before source terminal"
            ),
        }
    }
    async fn early(&mut self) {
        match self {
            Self::H1 { socket, prefix } => {
                tokio::time::timeout(WAIT, async {
                    loop {
                        let mut bytes = [0; 4096];
                        let n = socket.read(&mut bytes).await.unwrap();
                        assert!(n > 0);
                        prefix.extend_from_slice(&bytes[..n]);
                        if let Some(end) = prefix.windows(4).position(|w| w == b"\r\n\r\n").map(|n| n + 4) {
                            let header = String::from_utf8_lossy(&prefix[..end]);
                            assert!(header.starts_with("HTTP/1.1 200"));
                            let body = &prefix[end..];
                            if field(&header, "transfer-encoding").eq_ignore_ascii_case("chunked") {
                                if body
                                    .windows(2)
                                    .position(|w| w == b"\r\n")
                                    .is_some_and(|n| body.len() > n + 2)
                                {
                                    break;
                                }
                            } else if !body.is_empty() {
                                break;
                            }
                        }
                    }
                })
                .await
                .expect("positive control did not deliver early body bytes");
            }
            Self::H2 {
                response, body, prefix, ..
            } => {
                let head = tokio::time::timeout(WAIT, response.take().unwrap())
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(head.status(), 200);
                let mut stream = head.into_body();
                let chunk = tokio::time::timeout(WAIT, stream.data())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                assert!(!chunk.is_empty());
                stream.flow_control().release_capacity(chunk.len()).unwrap();
                prefix.extend_from_slice(&chunk);
                *body = Some(stream);
            }
        }
    }
    async fn blocked_output(&mut self) {
        let Self::H2 {
            response, body, prefix, ..
        } = self
        else {
            panic!("flow-control probe requires H2");
        };
        let head = tokio::time::timeout(WAIT, response.take().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(head.status(), 200);
        let mut stream = head.into_body();
        let mut held = 0;
        loop {
            match tokio::time::timeout(NO_HEAD, stream.data()).await {
                Ok(Some(Ok(chunk))) => {
                    held += chunk.len();
                    prefix.extend_from_slice(&chunk);
                }
                Err(_) => break,
                other => panic!("large response completed before flow-control block: {other:?}"),
            }
        }
        assert!(held > 0 && held <= 1024, "unexpected unacknowledged flow bytes: {held}");
        *body = Some(stream);
    }
    fn resume_flow(&mut self) {
        let Self::H2 { body, prefix, .. } = self else {
            return;
        };
        body.as_mut()
            .unwrap()
            .flow_control()
            .release_capacity(prefix.len())
            .unwrap();
    }
    async fn finish(self) -> (u16, Vec<u8>) {
        self.finish_checked(false).await
    }
    async fn finish_checked(self, expect_trailers: bool) -> (u16, Vec<u8>) {
        match self {
            Self::H1 { mut socket, mut prefix } => {
                tokio::time::timeout(WAIT, socket.read_to_end(&mut prefix))
                    .await
                    .unwrap()
                    .unwrap();
                let end = prefix
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .expect("missing final head")
                    + 4;
                if expect_trailers {
                    assert!(
                        String::from_utf8_lossy(&prefix[end..])
                            .to_ascii_lowercase()
                            .contains("x-result: complete\r\n"),
                        "valid H1 trailers were lost"
                    );
                }
                let header = String::from_utf8_lossy(&prefix[..end]);
                let status = header.split_whitespace().nth(1).unwrap().parse().unwrap();
                let body = if field(&header, "transfer-encoding").eq_ignore_ascii_case("chunked") {
                    decode_chunked(&prefix[end..])
                } else {
                    prefix[end..].to_vec()
                };
                (status, body)
            }
            Self::H2 {
                response,
                body,
                mut prefix,
                sender,
                driver,
            } => {
                let (status, mut body) = if let Some(body) = body {
                    (200, body)
                } else {
                    let head = tokio::time::timeout(WAIT, response.unwrap()).await.unwrap().unwrap();
                    (head.status().as_u16(), head.into_body())
                };
                while let Some(chunk) = tokio::time::timeout(WAIT, body.data()).await.unwrap() {
                    let chunk = chunk.unwrap();
                    body.flow_control().release_capacity(chunk.len()).unwrap();
                    prefix.extend_from_slice(&chunk);
                }
                if expect_trailers {
                    let trailers = tokio::time::timeout(WAIT, body.trailers())
                        .await
                        .unwrap()
                        .unwrap()
                        .expect("valid H2 trailers were lost");
                    assert_eq!(trailers["x-result"], "complete");
                }
                drop(sender);
                driver.abort();
                let _ = driver.await;
                (status, prefix)
            }
        }
    }
    async fn cancel(self) {
        match self {
            Self::H1 { socket, .. } => drop(socket),
            Self::H2 {
                response, body, driver, ..
            } => {
                drop(response);
                drop(body);
                driver.abort();
                let _ = driver.await;
            }
        }
    }
}
fn decode_chunked(mut wire: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    loop {
        let end = wire.windows(2).position(|w| w == b"\r\n").unwrap();
        let len = usize::from_str_radix(
            std::str::from_utf8(&wire[..end]).unwrap().split(';').next().unwrap(),
            16,
        )
        .unwrap();
        if len == 0 {
            return body;
        }
        wire = &wire[end + 2..];
        body.extend_from_slice(&wire[..len]);
        wire = &wire[len + 2..];
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_edgion_unary_hold_h1_h2_matrix() {
    let mut config = edgion_gateway::ai::config::AiConfig::default();
    config.conversion.chat_to_anthropic.unary.hold_timeout = "500ms".into();
    config.conversion.chat_to_anthropic.unary.memory_budget = "25MiB".into();
    config.conversion.chat_to_anthropic.unary.max_metadata_size = "8KiB".into();
    edgion_gateway::ai::config::initialize_for_test(&config).unwrap();
    let mut harness = Harness::new().await;
    for upstream_h2 in [false, true] {
        harness.h2.store(upstream_h2, Ordering::Release);
        for downstream_h2 in [false, true] {
            let mut reset_connection = None;
            for mode in [
                "success",
                "off",
                "off-gzip",
                "fragment",
                "interleaved-fragment",
                "input-overflow",
                "output-overflow",
                "truncated",
                "trickle",
                "trailers",
                "trailer-cumulative",
                "malformed",
                "reset",
                "wrong-media",
                "gzip",
                "trailer-overflow",
                "timeout",
            ] {
                let id = format!("{upstream_h2}-{downstream_h2}-{mode}");
                let mut client = Client::start(harness.port, downstream_h2, &id, mode, false).await;
                let mut event = harness.event(&id).await;
                let observed_head = std::time::Instant::now();
                assert_eq!(event.protocol, if upstream_h2 { "h2" } else { "h1" });
                if let Some(previous) = reset_connection.take() {
                    if upstream_h2 {
                        assert_eq!(event.conn, previous, "H2 source reset poisoned the connection");
                    } else {
                        assert_ne!(event.conn, previous, "H1 truncated connection was reused");
                    }
                }
                if mode == "reset" {
                    reset_connection = Some(event.conn);
                }
                if !matches!(mode, "wrong-media" | "gzip" | "off-gzip") {
                    client.no_head().await;
                }
                let stalled = if mode != "timeout" {
                    let _ = event.release.send(());
                    None
                } else {
                    Some(event.release)
                };
                let (status, body) = client.finish().await;
                drop(stalled);
                if mode == "trickle" {
                    let mut count = 0;
                    while event.progress.try_recv().is_ok() {
                        count += 1;
                    }
                    assert!(count >= 3, "absolute timeout had no progressing source");
                    assert!(
                        observed_head.elapsed() < Duration::from_secs(1),
                        "progress reset absolute Hold timeout"
                    );
                    eprintln!(
                        "wire trickle {id}: flushed_chunks={count}, elapsed={:?}",
                        observed_head.elapsed()
                    );
                }
                let expected = if matches!(mode, "timeout" | "trickle") {
                    504
                } else if matches!(
                    mode,
                    "success" | "off" | "fragment" | "interleaved-fragment" | "trailers"
                ) {
                    200
                } else {
                    502
                };
                assert_eq!(status, expected, "{id}: {}", String::from_utf8_lossy(&body));
                let parsed: Value = serde_json::from_slice(&body).unwrap();
                if expected == 200 {
                    assert_eq!(parsed["choices"][0]["message"]["content"], "hello wire");
                    if mode == "interleaved-fragment" {
                        let tool = &parsed["choices"][0]["message"]["tool_calls"][0];
                        assert_eq!(tool["id"], "tool_wire");
                        assert_eq!(tool["function"]["name"], "inspect");
                        assert_eq!(
                            serde_json::from_str::<Value>(tool["function"]["arguments"].as_str().unwrap()).unwrap(),
                            json!({"value":"hello\\world","enabled":true})
                        );
                        assert_eq!(parsed["choices"][0]["finish_reason"], "tool_calls");
                        assert_eq!(parsed["usage"]["prompt_tokens"], 15);
                        assert_eq!(parsed["usage"]["completion_tokens"], 11);
                    }
                } else {
                    assert_eq!(
                        parsed["error"]["code"],
                        if expected == 504 {
                            "upstream_timeout"
                        } else {
                            "upstream_response_invalid"
                        }
                    );
                    assert!(!body.windows(13).any(|w| w == b"must-not-leak"));
                }
                let log = harness.log(expected).await;
                if mode.starts_with("off") {
                    if mode == "off" {
                        assert_eq!(parsed["usage"]["prompt_tokens"], 7);
                        assert_eq!(parsed["usage"]["completion_tokens"], 11);
                    }
                    assert_eq!(log["genai"]["usage_source"], "notObserved");
                    assert_eq!(log["genai"]["attempts"][0]["usage_source"], "notObserved");
                    assert!(log["genai"]["input_tokens"].is_null());
                    assert!(log["genai"]["output_tokens"].is_null());
                }

                if matches!(
                    mode,
                    "success"
                        | "fragment"
                        | "interleaved-fragment"
                        | "output-overflow"
                        | "trailers"
                        | "trailer-overflow"
                        | "trailer-cumulative"
                ) {
                    assert_eq!(
                        log["genai"]["input_tokens"],
                        if mode == "interleaved-fragment" { 15 } else { 7 },
                        "validated input usage was lost: {log}"
                    );
                    assert_eq!(
                        log["genai"]["output_tokens"], 11,
                        "validated output usage was lost: {log}"
                    );
                    assert_eq!(log["genai"]["usage_estimated"], false);
                    if mode == "interleaved-fragment" {
                        assert_eq!(log["genai"]["cache_read_input_tokens"], 7);
                        assert_eq!(log["genai"]["cache_write_input_tokens"], 5);
                    }
                } else {
                    assert!(
                        log["genai"]["input_tokens"].is_null(),
                        "incomplete/invalid source reported exact usage: {log}"
                    );
                }
                assert_eq!(
                    log["backend_context"]["upstreams"][0]["status"], 200,
                    "raw provider status must survive client outcome: {log}"
                );
            }
        }
    }
    for upstream_h2 in [false, true] {
        harness.h2.store(upstream_h2, Ordering::Release);
        for downstream_h2 in [false, true] {
            for mode in ["preheader-eof", "preheader-timeout", "trailers-bandwidth"] {
                let id = format!("fix-{upstream_h2}-{downstream_h2}-{mode}");
                let mut client = Client::start(harness.port, downstream_h2, &id, mode, false).await;
                let event = harness.event(&id).await;
                assert_eq!(event.protocol, if upstream_h2 { "h2" } else { "h1" });
                client.no_head().await;
                let released_at = std::time::Instant::now();
                let stalled = if mode == "preheader-timeout" {
                    Some(event.release)
                } else {
                    let _ = event.release.send(());
                    None
                };
                let (status, body) = client.finish_checked(mode == "trailers-bandwidth").await;
                drop(stalled);
                let expected = match mode {
                    "preheader-eof" => 502,
                    "preheader-timeout" => 504,
                    _ => 200,
                };
                assert_eq!(status, expected, "{id}: {}", String::from_utf8_lossy(&body));
                let parsed: Value = serde_json::from_slice(&body).unwrap();
                if mode == "trailers-bandwidth" {
                    assert_eq!(parsed["choices"][0]["message"]["content"], "hello wire");
                    let expected_delay = Duration::from_secs_f64(body.len() as f64 / 1024.0);
                    assert!(
                        released_at.elapsed() + Duration::from_millis(10) >= expected_delay,
                        "converted trailer bytes escaped throttling: {id}"
                    );
                } else {
                    assert_eq!(
                        parsed["error"]["code"],
                        if expected == 504 {
                            "upstream_timeout"
                        } else {
                            "upstream_response_invalid"
                        }
                    );
                }
                let log = harness.log(expected).await;
                if mode == "trailers-bandwidth" {
                    assert_eq!(log["genai"]["input_tokens"], 7);
                    assert_eq!(log["genai"]["output_tokens"], 11);
                    assert_eq!(log["backend_context"]["upstreams"][0]["status"], 200);
                } else {
                    assert!(log["genai"]["input_tokens"].is_null());
                    assert_eq!(
                        log["backend_context"]["upstreams"].as_array().unwrap().len(),
                        1,
                        "preheader failure replayed model request: {log}"
                    );
                }
            }
        }
    }
    for upstream_h2 in [false, true] {
        harness.h2.store(upstream_h2, Ordering::Release);
        for cancel in [false, true] {
            let id = format!("slow-{upstream_h2}-{cancel}");
            let mut slow = Client::start(harness.port, true, &id, "slow", false).await;
            let event = harness.event(&id).await;
            event.release.send(()).unwrap();
            tokio::time::timeout(WAIT, event.eos)
                .await
                .expect("provider EOS was not flushed")
                .unwrap();
            slow.blocked_output().await;
            assert!(
                harness.logs.try_recv().is_err(),
                "writer completed despite withheld H2 capacity"
            );
            let rejected = Client::start(harness.port, true, "slow-saturated", "success", false).await;
            assert_eq!(rejected.finish().await.0, 503);
            let log = harness.log(503).await;
            assert!(
                log["backend_context"].is_null()
                    || log["backend_context"]["upstreams"].as_array().is_none_or(Vec::is_empty)
            );
            assert!(
                harness.started.try_recv().is_err(),
                "queued output lost reservation before delivery"
            );
            let completed = if cancel {
                slow.cancel().await;
                tokio::time::timeout(WAIT, harness.logs.recv()).await.unwrap().unwrap()
            } else {
                slow.resume_flow();
                let (status, body) = slow.finish().await;
                assert_eq!(status, 200);
                assert_eq!(
                    serde_json::from_slice::<Value>(&body).unwrap()["choices"][0]["message"]["content"]
                        .as_str()
                        .unwrap()
                        .len(),
                    800_000
                );
                harness.log(200).await
            };
            assert_eq!(completed["genai"]["input_tokens"], 7);
            assert_eq!(completed["genai"]["output_tokens"], 11);
            let id = format!("slow-readmitted-{upstream_h2}-{cancel}");
            let next = Client::start(harness.port, true, &id, "success", false).await;
            let event = harness.event(&id).await;
            event.release.send(()).unwrap();
            assert_eq!(next.finish().await.0, 200);
            harness.log(200).await;
            eprintln!("wire blocked output upstream_h2={upstream_h2}, cancel={cancel}: providerEOS+200+flowblock,503zeroattempt,readmitted");
        }
    }
    for upstream_h2 in [false, true] {
        harness.h2.store(upstream_h2, Ordering::Release);
        for downstream_h2 in [false, true] {
            for (mode, native) in [
                ("native", true),
                ("native-sse", true),
                ("ordinary", false),
                ("sse", false),
            ] {
                let id = format!("early-{upstream_h2}-{downstream_h2}-{mode}");
                let mut client = Client::start(harness.port, downstream_h2, &id, mode, native).await;
                let event = harness.event(&id).await;
                assert_eq!(
                    event.protocol,
                    if upstream_h2 && mode != "ordinary" { "h2" } else { "h1" }
                );
                eprintln!("wire positive control {id}");
                client.early().await;
                event.release.send(()).unwrap();
                let (status, body) = client.finish().await;
                assert_eq!(status, 200);
                if !matches!(mode, "sse" | "native-sse") {
                    assert_eq!(
                        serde_json::from_slice::<Value>(&body).unwrap()["choices"][0]["message"]["content"],
                        "native early"
                    );
                } else {
                    assert!(
                        body.windows(6).any(|b| b == b"[DONE]"),
                        "{id}: {}",
                        String::from_utf8_lossy(&body)
                    );
                }
                if mode == "native-sse" {
                    assert_eq!(body, fixture_body(mode));
                }
                let log = harness.log(200).await;
                if native {
                    assert_eq!(log["genai"]["usage_source"], "notObserved");
                    assert_eq!(log["genai"]["attempts"][0]["usage_source"], "notObserved");
                    assert!(log["genai"]["input_tokens"].is_null());
                }
            }
            let id = format!("held-{upstream_h2}-{downstream_h2}");
            let mut held = Client::start(harness.port, downstream_h2, &id, "success", false).await;
            let event = harness.event(&id).await;
            held.no_head().await;
            // Publishing fresh route/plugin/backend generations cannot create another pool.
            // The startup runtime is shared across all processed snapshots.
            publish(harness.port, harness.origin);
            let rejected = Client::start(harness.port, downstream_h2, "saturated", "success", false).await;
            let (status, body) = rejected.finish().await;
            assert_eq!(status, 503);
            assert!(String::from_utf8_lossy(&body).contains("service_unavailable"));
            let log = harness.log(503).await;
            assert!(
                log["backend_context"].is_null()
                    || log["backend_context"]["upstreams"].as_array().is_none_or(Vec::is_empty)
            );
            assert!(
                harness.started.try_recv().is_err(),
                "admission rejection dispatched a provider request"
            );
            if !upstream_h2 {
                let sibling_id = format!("sibling-{upstream_h2}-{downstream_h2}");
                let mut sibling = Client::start(harness.port, downstream_h2, &sibling_id, "sse", false).await;
                let sibling_event = harness.event(&sibling_id).await;
                assert_eq!(sibling_event.protocol, if upstream_h2 { "h2" } else { "h1" });
                // The selected peer's ordinary multiplexing policy may choose
                // another connection. Reuse after affected-stream reset is asserted below.
                sibling.early().await;
                sibling_event.release.send(()).unwrap();
                assert_eq!(sibling.finish().await.0, 200);
                harness.log(200).await;
            }
            event.release.send(()).unwrap();
            assert_eq!(held.finish().await.0, 200);
            harness.log(200).await;
            let id = format!("cancel-{upstream_h2}-{downstream_h2}");
            let mut canceled = Client::start(harness.port, downstream_h2, &id, "success", false).await;
            let previous = harness.event(&id).await;
            canceled.no_head().await;
            let canceled_at = std::time::Instant::now();
            canceled.cancel().await;
            tokio::time::timeout(Duration::from_millis(400), previous.closed)
                .await
                .expect("origin did not observe cancellation close/reset before Hold expiry")
                .unwrap();
            let terminal = tokio::time::timeout(WAIT, harness.logs.recv()).await.unwrap().unwrap();
            eprintln!(
                "wire cancellation upstream_h2={upstream_h2} downstream_h2={downstream_h2}: status={}, elapsed={:?}",
                terminal["request_info"]["status"],
                canceled_at.elapsed()
            );
            assert!(
                canceled_at.elapsed() < Duration::from_millis(400),
                "cancellation waited for configured Hold expiry"
            );
            assert_ne!(
                terminal["request_info"]["status"], 200,
                "cancellation was logged as successful delivery"
            );
            let id = format!("after-cancel-{upstream_h2}-{downstream_h2}");
            let next = Client::start(harness.port, downstream_h2, &id, "success", false).await;
            let event = harness.event(&id).await;
            if upstream_h2 {
                assert_eq!(
                    event.conn, previous.conn,
                    "H2 cancellation poisoned the origin connection"
                );
            } else {
                assert_ne!(event.conn, previous.conn, "H1 incomplete connection was reused");
            }
            drop(previous.release);
            event.release.send(()).unwrap();
            assert_eq!(next.finish().await.0, 200);
            harness.log(200).await;
        }
    }
    harness.close().await;
}
