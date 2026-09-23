use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::plugins::http::common::outbound::request::CompiledRequest;
use crate::plugins::runtime::traits::session::MockPluginSession;
use edgion_resources::resources::common::outbound_request::OutboundRequestSpec;
use edgion_resources::resources::common::request_template::CompiledTemplate;
use edgion_resources::resources::common::success::{BodyPredicate, PredicateOp, SuccessSpec};

#[derive(Clone, Copy, Debug)]
enum FakeError {
    Timeout,
    Connect,
    Other,
}

impl OutboundTransportError for FakeError {
    fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout)
    }

    fn is_connect(&self) -> bool {
        matches!(self, Self::Connect)
    }
}

struct FakeResponse {
    status: u16,
    headers: reqwest::header::HeaderMap,
    content_length: Option<u64>,
    chunks: Vec<Result<Bytes, FakeError>>,
    body_stream_called: Option<Arc<AtomicBool>>,
}

impl FakeResponse {
    fn new(status: u16, body: impl Into<Bytes>) -> Self {
        let body = body.into();
        Self {
            status,
            headers: reqwest::header::HeaderMap::new(),
            content_length: Some(body.len() as u64),
            chunks: vec![Ok(body)],
            body_stream_called: None,
        }
    }

    fn header(mut self, name: &'static str, value: &'static str) -> Self {
        self.headers.insert(name, value.parse().unwrap());
        self
    }

    fn streamed(mut self, chunks: Vec<Result<Bytes, FakeError>>) -> Self {
        self.content_length = None;
        self.chunks = chunks;
        self
    }

    fn reported_length(mut self, length: u64) -> Self {
        self.content_length = Some(length);
        self
    }

    fn observe_body_stream(mut self, signal: Arc<AtomicBool>) -> Self {
        self.body_stream_called = Some(signal);
        self
    }
}

impl OutboundHttpResponse for FakeResponse {
    type BodyError = FakeError;

    fn status(&self) -> u16 {
        self.status
    }

    fn headers(&self) -> &reqwest::header::HeaderMap {
        &self.headers
    }

    fn content_length(&self) -> Option<u64> {
        self.content_length
    }

    fn bytes_stream(self) -> impl Stream<Item = Result<Bytes, Self::BodyError>> {
        if let Some(signal) = self.body_stream_called {
            signal.store(true, Ordering::SeqCst);
        }
        futures::stream::iter(self.chunks)
    }
}

struct FakeStep {
    delay: Duration,
    result: Result<FakeResponse, FakeError>,
    drop_signal: Option<Arc<AtomicBool>>,
}

impl From<FakeResponse> for FakeStep {
    fn from(response: FakeResponse) -> Self {
        Self {
            delay: Duration::ZERO,
            result: Ok(response),
            drop_signal: None,
        }
    }
}

impl FakeStep {
    fn delayed(response: FakeResponse, delay: Duration) -> Self {
        Self {
            delay,
            result: Ok(response),
            drop_signal: None,
        }
    }

    fn error(error: FakeError) -> Self {
        Self {
            delay: Duration::ZERO,
            result: Err(error),
            drop_signal: None,
        }
    }
}

struct RecordedRequest {
    method: reqwest::Method,
    path: String,
    headers: reqwest::header::HeaderMap,
    body: Option<Bytes>,
    timeout: Option<Duration>,
}

struct FakeTransport {
    steps: Mutex<VecDeque<FakeStep>>,
    requests: Mutex<Vec<RecordedRequest>>,
    expected_route: Option<(reqwest::Method, String)>,
}

impl FakeTransport {
    fn new(steps: impl IntoIterator<Item = FakeStep>) -> Self {
        Self {
            steps: Mutex::new(steps.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
            expected_route: None,
        }
    }

    fn for_route(method: reqwest::Method, path: &str, steps: impl IntoIterator<Item = FakeStep>) -> Self {
        let mut transport = Self::new(steps);
        transport.expected_route = Some((method, path.to_string()));
        transport
    }

    fn count(&self) -> usize {
        self.requests.lock().len()
    }

    fn assert_drained(&self) {
        assert!(self.steps.lock().is_empty(), "unused fake responses");
    }
}

impl OutboundTransport for FakeTransport {
    type Response = FakeResponse;
    type Error = FakeError;

    async fn execute(&self, req: reqwest::Request) -> Result<Self::Response, Self::Error> {
        if let Some((method, path)) = &self.expected_route {
            assert_eq!(req.method(), method);
            assert_eq!(req.url().path(), path);
        }
        self.requests.lock().push(RecordedRequest {
            method: req.method().clone(),
            path: req.url().path().to_string(),
            headers: req.headers().clone(),
            body: req.body().and_then(|body| body.as_bytes()).map(Bytes::copy_from_slice),
            timeout: req.timeout().copied(),
        });
        let step = self.steps.lock().pop_front().expect("unexpected outbound attempt");
        struct DropSignal(Arc<AtomicBool>);
        impl Drop for DropSignal {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let _drop_signal = step.drop_signal.map(DropSignal);
        tokio::time::sleep(step.delay).await;
        step.result
    }
}

fn make_client() -> ManagedOutboundClient {
    ManagedOutboundClient::test_with_static_target("http://10.0.0.1:8080")
}

fn session(method: &str, path: &str) -> MockPluginSession {
    let mut session = MockPluginSession::new();
    session.expect_get_method().return_const(method.to_string());
    session.expect_get_request_path().return_const(path.to_string());
    session
}

async fn call(
    client: &ManagedOutboundClient,
    session: &dyn PluginSession,
    transport: &FakeTransport,
    global_max_bytes: usize,
) -> Result<OutboundResponse, OutboundCallError> {
    outbound_call_with_body_using(
        client,
        session,
        None,
        global_max_bytes,
        &[],
        None,
        &mut Default::default(),
        transport,
    )
    .await
}

#[tokio::test]
async fn non_success_returns_response_with_headers() {
    let transport = FakeTransport::for_route(
        reqwest::Method::GET,
        "/check",
        [FakeResponse::new(403, Bytes::from_static(b"denied"))
            .header("www-authenticate", "Bearer")
            .into()],
    );
    let response = call(&make_client(), &session("GET", "/check"), &transport, 4096)
        .await
        .unwrap();
    assert_eq!(response.status, 403);
    assert_eq!(response.headers["www-authenticate"], "Bearer");
    assert_eq!(response.body, Bytes::from_static(b"denied"));
    assert_eq!(transport.requests.lock()[0].path, "/check");
}

#[tokio::test(start_paused = true)]
async fn timeout_returns_timeout_error() {
    let mut client = make_client();
    client.timeout = Duration::from_millis(50);
    let transport = FakeTransport::for_route(
        reqwest::Method::GET,
        "/slow",
        [FakeStep::delayed(
            FakeResponse::new(200, Bytes::from_static(b"eventually")),
            Duration::from_millis(500),
        )],
    );
    assert!(matches!(
        call(&client, &session("GET", "/slow"), &transport, 4096).await,
        Err(OutboundCallError::Timeout)
    ));
    assert_eq!(transport.count(), 1);
    assert_eq!(transport.requests.lock()[0].timeout, Some(Duration::from_millis(50)));
}

#[tokio::test]
async fn unhealthy_gate_returns_unhealthy_error() {
    let client = make_client();
    client.health.healthy.store(false, Ordering::Relaxed);
    let transport = FakeTransport::new([]);
    assert!(matches!(
        call(&client, &MockPluginSession::new(), &transport, 4096).await,
        Err(OutboundCallError::Unhealthy)
    ));
    assert_eq!(transport.count(), 0);
}

#[tokio::test]
async fn rate_limited_gate_returns_rate_limited_error() {
    use crate::link_sys::providers::webhook::runtime::SlidingWindowCounter;

    let mut client = make_client();
    client.rate = Some(SlidingWindowCounter::new(0, Duration::from_secs(1)));
    let transport = FakeTransport::new([]);
    assert!(matches!(
        call(&client, &MockPluginSession::new(), &transport, 4096).await,
        Err(OutboundCallError::RateLimited)
    ));
    assert_eq!(transport.count(), 0);
}

#[tokio::test]
async fn response_too_large_returns_error() {
    let mut client = make_client();
    client.max_response_bytes = 10;
    let transport = FakeTransport::for_route(
        reqwest::Method::GET,
        "/big",
        [FakeResponse::new(200, vec![b'x'; 200]).into()],
    );
    assert!(matches!(
        call(&client, &session("GET", "/big"), &transport, 65536).await,
        Err(OutboundCallError::ResponseTooLarge)
    ));
}

#[tokio::test]
async fn successful_200_returns_ok_with_body() {
    let transport = FakeTransport::for_route(
        reqwest::Method::GET,
        "/ok",
        [FakeResponse::new(200, Bytes::from_static(b"hello")).into()],
    );
    let response = call(&make_client(), &session("GET", "/ok"), &transport, 65536)
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, Bytes::from_static(b"hello"));
    assert_eq!(transport.count(), 1);
}

#[tokio::test]
async fn response_body_cap_streams_exact_limit_and_rejects_overflow() {
    let mut client = make_client();
    client.max_response_bytes = 5;
    let exact = FakeResponse::new(200, Bytes::new())
        .streamed(vec![Ok(Bytes::from_static(b"ab")), Ok(Bytes::from_static(b"cde"))]);
    let overflow = FakeResponse::new(200, Bytes::new())
        .streamed(vec![Ok(Bytes::from_static(b"abc")), Ok(Bytes::from_static(b"def"))]);
    let exact_transport = FakeTransport::new([exact.into()]);
    let response = call(&client, &session("GET", "/exact"), &exact_transport, 100)
        .await
        .unwrap();
    assert_eq!(response.body, Bytes::from_static(b"abcde"));
    let overflow_transport = FakeTransport::new([overflow.into()]);
    assert!(matches!(
        call(&client, &session("GET", "/overflow"), &overflow_transport, 100).await,
        Err(OutboundCallError::ResponseTooLarge)
    ));
    let false_length = FakeResponse::new(200, Bytes::new())
        .streamed(vec![Ok(Bytes::from_static(b"abcdef"))])
        .reported_length(3);
    let false_length_transport = FakeTransport::new([false_length.into()]);
    assert!(matches!(
        call(&client, &session("GET", "/false-length"), &false_length_transport, 100).await,
        Err(OutboundCallError::ResponseTooLarge)
    ));
}

#[tokio::test]
async fn response_body_stream_error_is_connect_failure() {
    let response =
        FakeResponse::new(200, Bytes::new()).streamed(vec![Ok(Bytes::from_static(b"ok")), Err(FakeError::Other)]);
    let transport = FakeTransport::new([response.into()]);
    assert!(matches!(
        call(&make_client(), &session("GET", "/broken"), &transport, 4096).await,
        Err(OutboundCallError::Connect)
    ));
}

#[tokio::test]
async fn retry_on_status_retries_and_succeeds() {
    let mut client = make_client();
    let mut retry = tests_retry(0);
    retry.max_retries = 1;
    retry.retry_on_status = vec![503];
    client.retry = Some(retry);
    let transport = FakeTransport::for_route(
        reqwest::Method::GET,
        "/retry",
        [
            FakeResponse::new(503, Bytes::new()).into(),
            FakeResponse::new(200, Bytes::from_static(b"ok")).into(),
        ],
    );
    let response = call(&client, &session("GET", "/retry"), &transport, 65536)
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, Bytes::from_static(b"ok"));
    assert_eq!(transport.count(), 2);
    transport.assert_drained();
}

#[tokio::test(start_paused = true)]
async fn retry_after_within_cap_is_honored() {
    let mut client = make_client();
    let mut retry = tests_retry(0);
    retry.retry_on_status = vec![503];
    retry.honor_retry_after = true;
    retry.max_delay_ms = 2000;
    client.retry = Some(retry);
    let transport = FakeTransport::for_route(
        reqwest::Method::GET,
        "/ra",
        [
            FakeResponse::new(503, Bytes::new()).header("retry-after", "1").into(),
            FakeResponse::new(200, Bytes::new()).into(),
        ],
    );
    let started = tokio::time::Instant::now();
    let response = call(&client, &session("GET", "/ra"), &transport, 4096).await.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(tokio::time::Instant::now() - started, Duration::from_secs(1));
    assert_eq!(transport.count(), 2);
}

#[tokio::test]
async fn retry_after_over_cap_stops_and_returns_response() {
    let mut client = make_client();
    let mut retry = tests_retry(0);
    retry.retry_on_status = vec![503];
    retry.honor_retry_after = true;
    retry.max_delay_ms = 2000;
    client.retry = Some(retry);
    let transport = FakeTransport::for_route(
        reqwest::Method::GET,
        "/ra2",
        [FakeResponse::new(503, Bytes::new())
            .header("retry-after", "3600")
            .into()],
    );
    let response = call(&client, &session("GET", "/ra2"), &transport, 4096).await.unwrap();
    assert_eq!(response.status, 503);
    assert_eq!(transport.count(), 1);
}

#[test]
fn retry_after_http_date_uses_fixed_clock() {
    let now = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let future = httpdate::fmt_http_date(now + Duration::from_secs(30));
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::RETRY_AFTER, future.parse().unwrap());
    assert_eq!(parse_retry_after_ms(&headers, now), Some(30_000));
    assert_eq!(parse_retry_after_ms(&headers, now + Duration::from_secs(40)), Some(0));
}

fn body_success_spec() -> SuccessSpec {
    SuccessSpec {
        status_codes: Some(vec![200]),
        body: vec![BodyPredicate {
            pointer: "/code".into(),
            op: PredicateOp::Equals(serde_json::json!(0)),
        }],
    }
}

fn json_response(code: i32) -> FakeResponse {
    FakeResponse::new(200, Bytes::from(format!(r#"{{"code":{code}}}"#))).header("content-type", "application/json")
}

#[tokio::test]
async fn body_failure_returns_response_and_optionally_retries() {
    let mut client = make_client();
    client.success = Some(body_success_spec());
    let transport = FakeTransport::for_route(reqwest::Method::GET, "/biz", [json_response(-101).into()]);
    let response = call(&client, &session("GET", "/biz"), &transport, 4096).await.unwrap();
    assert_eq!(response.status, 200);
    assert!(!client.classify(&response));
    assert_eq!(transport.count(), 1);
}

#[tokio::test]
async fn terminal_body_fail_records_predicate_fail_metric() {
    use crate::observe::metrics::registry::names;
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};

    let mut client = make_client();
    client.success = Some(body_success_spec());
    let transport = FakeTransport::for_route(reqwest::Method::GET, "/biz-metric", [json_response(-101).into()]);
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    {
        let _guard = metrics::set_default_local_recorder(&recorder);
        let response = call(&client, &session("GET", "/biz-metric"), &transport, 4096)
            .await
            .unwrap();
        assert_eq!(response.status, 200);
    }
    let count = snapshotter
        .snapshot()
        .into_vec()
        .iter()
        .find(|(key, _, _, _)| key.key().name() == names::OUTBOUND_SUCCESS_PREDICATE_FAIL_TOTAL)
        .map(|(_, _, _, value)| match value {
            DebugValue::Counter(count) => *count,
            other => panic!("expected counter, got {other:?}"),
        })
        .expect("terminal body failure must emit the metric");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn status_retry_does_not_stream_oversized_error_body() {
    use edgion_resources::resources::common::outbound_request::PresencePolicy;

    let mut client = make_client();
    client.max_response_bytes = 16;
    let mut retry = tests_retry(0);
    retry.retry_on_status = vec![503];
    client.retry = Some(retry);
    client
        .compiled_request
        .headers
        .origin
        .push(super::super::request::CompiledOriginField {
            name: "x-private-metadata".into(),
            presence: PresencePolicy::WarnIfMissing,
        });
    let mut session = session("GET", "/retry-big");
    session.expect_header_value().times(1).return_const(None::<String>);
    let first_body_stream_called = Arc::new(AtomicBool::new(false));
    let transport = FakeTransport::for_route(
        reqwest::Method::GET,
        "/retry-big",
        [
            FakeResponse::new(503, vec![b'x'; 64])
                .observe_body_stream(Arc::clone(&first_body_stream_called))
                .into(),
            FakeResponse::new(200, Bytes::from_static(b"ok")).into(),
        ],
    );
    let mut diagnostics = OutboundRequestDiagnostics::default();
    let response =
        outbound_call_with_body_using(&client, &session, None, 65536, &[], None, &mut diagnostics, &transport)
            .await
            .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(transport.count(), 2);
    assert!(!first_body_stream_called.load(Ordering::SeqCst));
    assert!(transport
        .requests
        .lock()
        .iter()
        .all(|request| !request.headers.contains_key("x-private-metadata")));
    let mut log = crate::plugins::runtime::PluginLog::new("test");
    diagnostics.emit(&mut log);
    assert_eq!(log.render_for_test(), "warn outbound field missing");
}

#[tokio::test]
async fn retry_on_body_failure_actually_retries() {
    let mut client = make_client();
    client.success = Some(body_success_spec());
    let mut retry = tests_retry(0);
    retry.retry_on_body_failure = true;
    client.retry = Some(retry);
    let transport = FakeTransport::for_route(
        reqwest::Method::GET,
        "/biz-retry",
        [json_response(-101).into(), json_response(0).into()],
    );
    let response = call(&client, &session("GET", "/biz-retry"), &transport, 4096)
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert!(client.classify(&response));
    assert_eq!(transport.count(), 2);
}

#[tokio::test]
async fn retry_on_body_failure_does_not_widen_to_status_failures() {
    let mut client = make_client();
    let mut retry = tests_retry(0);
    retry.retry_on_body_failure = true;
    client.retry = Some(retry);
    let transport = FakeTransport::for_route(
        reqwest::Method::GET,
        "/not-found",
        [FakeResponse::new(404, Bytes::new()).into()],
    );
    let response = call(&client, &session("GET", "/not-found"), &transport, 4096)
        .await
        .unwrap();
    assert_eq!(response.status, 404);
    assert_eq!(transport.count(), 1);
}

#[tokio::test]
async fn retry_after_over_cap_suppresses_body_retry() {
    let mut client = make_client();
    let mut retry = tests_retry(0);
    retry.retry_on_status = vec![503];
    retry.retry_on_body_failure = true;
    retry.honor_retry_after = true;
    retry.max_delay_ms = 2000;
    client.retry = Some(retry);
    let transport = FakeTransport::for_route(
        reqwest::Method::GET,
        "/ra-body",
        [FakeResponse::new(503, Bytes::new())
            .header("retry-after", "3600")
            .into()],
    );
    let response = call(&client, &session("GET", "/ra-body"), &transport, 4096)
        .await
        .unwrap();
    assert_eq!(response.status, 503);
    assert_eq!(transport.count(), 1);
}

#[tokio::test(start_paused = true)]
async fn timeout_not_retried_by_default_when_option_none() {
    let mut client = make_client();
    client.timeout = Duration::from_millis(50);
    let mut retry = tests_retry(0);
    retry.retry_on_timeout = None;
    client.retry = Some(retry);
    let transport = FakeTransport::for_route(
        reqwest::Method::GET,
        "/slow",
        [FakeStep::delayed(
            FakeResponse::new(200, Bytes::new()),
            Duration::from_millis(300),
        )],
    );
    assert!(matches!(
        call(&client, &session("GET", "/slow"), &transport, 4096).await,
        Err(OutboundCallError::Timeout)
    ));
    assert_eq!(transport.count(), 1);
}

#[tokio::test]
async fn forward_all_actually_forwards_authorization_header() {
    use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
    use edgion_resources::resources::common::outbound_request::ListFields;

    let mut client = make_client();
    client.compiled_request = CompiledRequest::from_spec(&OutboundRequestSpec {
        headers: ListFields {
            forward_all: true,
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap();
    let mut session = session("GET", "/verify");
    setup_header_mocks(&mut session, &[("authorization", "Bearer test-token")]);
    let transport = FakeTransport::for_route(
        reqwest::Method::GET,
        "/verify",
        [FakeResponse::new(200, Bytes::new()).into()],
    );
    assert_eq!(call(&client, &session, &transport, 4096).await.unwrap().status, 200);
    assert_eq!(
        transport.requests.lock()[0].headers["authorization"],
        "Bearer test-token"
    );
}

#[tokio::test]
async fn secret_ref_in_custom_header_is_resolved_at_runtime() {
    use edgion_resources::resources::common::external_http::ResolvedSecrets;
    use edgion_resources::resources::common::outbound_request::{CustomField, CustomSource, ListFields};

    let mut client = make_client();
    client.compiled_request = CompiledRequest::from_spec(&OutboundRequestSpec {
        headers: ListFields {
            custom: vec![CustomField {
                name: "authorization".to_string(),
                source: CustomSource::Template("Bearer ${secretRef:ns/n:k}".to_string()),
            }],
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap();
    client.resolved_secrets = Some(ResolvedSecrets(std::collections::BTreeMap::from([(
        "ns/n:k".to_string(),
        "test-secret-value".to_string(),
    )])));
    let transport = FakeTransport::for_route(
        reqwest::Method::GET,
        "/api",
        [FakeResponse::new(200, Bytes::new()).into()],
    );
    assert_eq!(
        call(&client, &session("GET", "/api"), &transport, 4096)
            .await
            .unwrap()
            .status,
        200
    );
    assert_eq!(
        transport.requests.lock()[0].headers["authorization"],
        "Bearer test-secret-value"
    );
}

#[tokio::test]
async fn json_body_with_secret_ref_is_sent() {
    use edgion_resources::resources::common::external_http::ResolvedSecrets;
    use edgion_resources::resources::common::outbound_request::{BodySpec, MethodSpec, PathSpec};

    let mut client = make_client();
    client.compiled_request = CompiledRequest::from_spec(&OutboundRequestSpec {
        path: Some(PathSpec {
            template: "/notify".into(),
            allow_override: false,
        }),
        method: Some(MethodSpec {
            template: "POST".into(),
        }),
        body: Some(BodySpec::Json {
            template: r#"{"token": "${secretRef:ns/n:k}", "caller": "edgion-gateway"}"#.into(),
        }),
        ..Default::default()
    })
    .unwrap();
    client.resolved_secrets = Some(ResolvedSecrets(std::collections::BTreeMap::from([(
        "ns/n:k".to_string(),
        "test-secret-value".to_string(),
    )])));
    let mut retry = tests_retry(0);
    retry.max_retries = 1;
    retry.retry_on_status = vec![503];
    client.retry = Some(retry);
    let transport = FakeTransport::for_route(
        reqwest::Method::POST,
        "/notify",
        [
            FakeResponse::new(503, Bytes::new()).into(),
            FakeResponse::new(200, Bytes::new()).into(),
        ],
    );
    assert_eq!(
        call(&client, &MockPluginSession::new(), &transport, 4096)
            .await
            .unwrap()
            .status,
        200
    );
    let requests = transport.requests.lock();
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        assert_eq!(request.method, reqwest::Method::POST);
        assert_eq!(request.path, "/notify");
        assert_eq!(request.headers["content-type"], "application/json");
        let body: serde_json::Value = serde_json::from_slice(request.body.as_ref().unwrap()).unwrap();
        assert_eq!(
            body,
            serde_json::json!({"token": "test-secret-value", "caller": "edgion-gateway"})
        );
        assert_eq!(
            request.headers["content-length"],
            request.body.as_ref().unwrap().len().to_string()
        );
    }
}

fn timeout_session(path: &str, header: Option<&'static str>) -> MockPluginSession {
    let mut session = session("GET", path);
    let mut headers = http::HeaderMap::new();
    if let Some(value) = header {
        headers.insert("X-Test-Timeout", http::HeaderValue::from_static(value));
    }
    session.expect_request_header_map().return_const(headers);
    session
}

#[tokio::test(start_paused = true)]
async fn timeout_template_from_header_triggers_timeout() {
    let mut client = make_client();
    client.timeout = Duration::from_secs(5);
    client.timeout_template = Some(CompiledTemplate::compile("${header:X-Test-Timeout}").unwrap());
    let transport = FakeTransport::for_route(
        reqwest::Method::GET,
        "/t",
        [FakeStep::delayed(
            FakeResponse::new(200, Bytes::new()),
            Duration::from_millis(200),
        )],
    );
    assert!(matches!(
        call(&client, &timeout_session("/t", Some("50")), &transport, 4096).await,
        Err(OutboundCallError::Timeout)
    ));
    assert_eq!(transport.requests.lock()[0].timeout, Some(Duration::from_millis(50)));
}

#[tokio::test(start_paused = true)]
async fn timeout_template_garbage_render_falls_back_to_static_timeout() {
    let mut client = make_client();
    client.timeout = Duration::from_secs(5);
    client.timeout_template = Some(CompiledTemplate::compile("${header:X-Test-Timeout}").unwrap());
    let transport = FakeTransport::for_route(
        reqwest::Method::GET,
        "/t2",
        [FakeStep::delayed(
            FakeResponse::new(200, Bytes::new()),
            Duration::from_millis(50),
        )],
    );
    assert_eq!(
        call(&client, &timeout_session("/t2", Some("abc")), &transport, 4096)
            .await
            .unwrap()
            .status,
        200
    );
    assert_eq!(transport.requests.lock()[0].timeout, Some(Duration::from_secs(5)));
}

#[tokio::test(start_paused = true)]
async fn timeout_template_missing_header_falls_back_to_static_timeout() {
    let mut client = make_client();
    client.timeout = Duration::from_secs(5);
    client.timeout_template = Some(CompiledTemplate::compile("${header:X-Test-Timeout}").unwrap());
    let transport = FakeTransport::for_route(
        reqwest::Method::GET,
        "/t3",
        [FakeResponse::new(200, Bytes::new()).into()],
    );
    assert_eq!(
        call(&client, &timeout_session("/t3", None), &transport, 4096)
            .await
            .unwrap()
            .status,
        200
    );
    assert_eq!(transport.requests.lock()[0].timeout, Some(Duration::from_secs(5)));
}

#[tokio::test(start_paused = true)]
async fn retry_on_connect_error_uses_bounded_backoff() {
    let mut client = make_client();
    let mut retry = tests_retry(100);
    retry.max_retries = 2;
    retry.max_delay_ms = 150;
    client.retry = Some(retry);
    let transport = FakeTransport::new([
        FakeStep::error(FakeError::Connect),
        FakeStep::error(FakeError::Connect),
        FakeResponse::new(200, Bytes::new()).into(),
    ]);
    let started = tokio::time::Instant::now();
    assert_eq!(
        call(&client, &session("GET", "/connect"), &transport, 4096)
            .await
            .unwrap()
            .status,
        200
    );
    assert_eq!(tokio::time::Instant::now() - started, Duration::from_millis(250));
    assert_eq!(transport.count(), 3);
}

#[tokio::test]
async fn transport_errors_keep_public_classification() {
    for (error, expected) in [
        (FakeError::Timeout, "timeout"),
        (FakeError::Connect, "connect"),
        (FakeError::Other, "connect"),
    ] {
        let transport = FakeTransport::new([FakeStep::error(error)]);
        let result = call(&make_client(), &session("GET", "/error"), &transport, 4096).await;
        match expected {
            "timeout" => assert!(matches!(result, Err(OutboundCallError::Timeout))),
            _ => assert!(matches!(result, Err(OutboundCallError::Connect))),
        }
        assert_eq!(transport.count(), 1);
    }
}

#[tokio::test]
async fn body_override_rebuilds_framing_before_dispatch() {
    let client = make_client();
    let transport = FakeTransport::new([FakeResponse::new(200, Bytes::new()).into()]);
    let body = Bytes::from_static(b"hello");
    let response = outbound_call_with_body_using(
        &client,
        &session("POST", "/framing"),
        None,
        4096,
        &[("transfer-encoding", "chunked"), ("content-length", "999")],
        Some(body.clone()),
        &mut Default::default(),
        &transport,
    )
    .await
    .unwrap();
    assert_eq!(response.status, 200);
    let requests = transport.requests.lock();
    assert_eq!(requests[0].body.as_ref(), Some(&body));
    assert_eq!(requests[0].headers["content-length"], "5");
    assert!(!requests[0].headers.contains_key("transfer-encoding"));
}

#[tokio::test(start_paused = true)]
async fn cancellation_drops_inflight_transport_attempt() {
    let client = make_client();
    let session = session("GET", "/cancel");
    let canceled = Arc::new(AtomicBool::new(false));
    let mut step = FakeStep::delayed(FakeResponse::new(200, Bytes::new()), Duration::from_secs(30));
    step.drop_signal = Some(Arc::clone(&canceled));
    let transport = FakeTransport::new([step]);
    let mut call = Box::pin(call(&client, &session, &transport, 4096));
    tokio::select! {
        biased;
        _ = &mut call => panic!("outbound call completed before cancellation"),
        _ = tokio::task::yield_now() => {}
    }
    assert_eq!(transport.count(), 1);
    drop(call);
    assert!(canceled.load(Ordering::SeqCst));
}
