use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use rand::Rng;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::sync::{mpsc, oneshot, Semaphore};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use crate::backends::{select_service_endpoint, service_section_key_from_port, validate_endpoint_backend_addr};
use crate::ctx::{MirrorAbandonReason, MirrorCancellation};
use crate::observe::metrics::record_mirror_metric;
use crate::plugins::http::common::http_client::{
    connection_listed_hop_by_hop, get_mirror_http_client, is_hop_by_hop_or_listed,
};
use crate::plugins::runtime::{PluginLog, PluginSession, PluginSessionHeaderExt, RequestFilter};
use crate::{MirrorConfig, MirrorState};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::http_route::{HTTPRequestMirrorFilter, MirrorTuning};

use super::mirror_log::{emit_mirror_log, new_entry};

pub struct RequestMirrorPlugin {
    config: HTTPRequestMirrorFilter,
    tuning: MirrorTuning,
    semaphore: Arc<Semaphore>,
}

async fn wait_for_send_or_cancel<F>(send_result: F, mut cancel_rx: oneshot::Receiver<()>) -> Option<F::Output>
where
    F: Future,
{
    tokio::pin!(send_result);
    tokio::select! {
        biased;
        cancelled = &mut cancel_rx => {
            if cancelled.is_ok() {
                None
            } else {
                Some(send_result.await)
            }
        }
        result = &mut send_result => Some(result),
    }
}

fn mirror_abandon_status(reason: &std::sync::atomic::AtomicU8) -> Option<&'static str> {
    match MirrorCancellation::load_reason(reason) {
        Some(MirrorAbandonReason::ChannelFull) => Some("channel_full"),
        Some(MirrorAbandonReason::BodyAbandoned) => Some("body_abandoned"),
        None => None,
    }
}

impl RequestMirrorPlugin {
    pub fn new(config: HTTPRequestMirrorFilter, tuning: MirrorTuning, default_namespace: String) -> Self {
        let mut cfg = config;
        let mut tuning = tuning;
        if tuning.max_buffered_chunks == 0 {
            tuning.max_buffered_chunks = 5;
        }
        if tuning.max_concurrent == 0 {
            tuning.max_concurrent = 1024;
        }
        // Hard cap: channel_full_timeout_ms must not exceed 1000ms.
        // Values above 1s would add unacceptable back-pressure to main request body processing.
        if tuning.channel_full_timeout_ms > 1000 {
            tracing::warn!(
                channel_full_timeout_ms = tuning.channel_full_timeout_ms,
                "channel_full_timeout_ms exceeds maximum (1000ms); clamping to 1000ms"
            );
            tuning.channel_full_timeout_ms = 1000;
        }

        // If backendRef.namespace is absent, default to current route/plugin namespace.
        if cfg.backend_ref.namespace.is_none() {
            cfg.backend_ref.namespace = Some(default_namespace);
        }

        Self {
            semaphore: Arc::new(Semaphore::new(tuning.max_concurrent)),
            config: cfg,
            tuning,
        }
    }

    /// Returns `Some(reason)` when mirroring should be skipped, `None` when it should proceed.
    /// Distinguishes config-disabled (zero rate) from random sampling rejection.
    fn skip_reason(&self) -> Option<&'static str> {
        if let Some(fraction) = &self.config.fraction {
            let denominator = fraction.denominator.unwrap_or(100);
            if denominator <= 0 || fraction.numerator <= 0 {
                return Some("mirror disabled");
            }
            if fraction.numerator >= denominator {
                return None;
            }
            let mut rng = rand::rng();
            let sample: i32 = rng.random_range(0..denominator);
            return if sample < fraction.numerator {
                None
            } else {
                Some("mirror sampled:skip")
            };
        }
        if let Some(pct) = self.config.percent {
            if pct <= 0 {
                return Some("mirror disabled");
            }
            if pct >= 100 {
                return None;
            }
            let mut rng = rand::rng();
            let sample: i32 = rng.random_range(0..100);
            return if sample < pct {
                None
            } else {
                Some("mirror sampled:skip")
            };
        }
        None
    }

    fn mirror_config(&self) -> MirrorConfig {
        MirrorConfig {
            connect_timeout: Duration::from_millis(self.tuning.connect_timeout_ms.max(1)),
            write_timeout: Duration::from_millis(self.tuning.write_timeout_ms.max(1)),
            max_buffered_chunks: self.tuning.max_buffered_chunks,
            mirror_log: self.tuning.mirror_log,
            // max_concurrent omitted: managed by self.semaphore, not needed inside the task.
        }
    }

    fn resolve_target(&self, allow_loopback_upstream: bool) -> Result<(String, String), String> {
        let port = self
            .config
            .backend_ref
            .port
            .ok_or_else(|| "backend_ref.port is required for RequestMirror".to_string())?;
        if !edgion_resources::gwapi_types::is_valid_port(port as i64) {
            return Err(format!(
                "backend_ref.port must be in {}..={}",
                edgion_resources::gwapi_types::PORT_MIN,
                edgion_resources::gwapi_types::PORT_MAX
            ));
        }
        if self.config.backend_ref.name.is_empty() {
            return Err("backend_ref.name is required for RequestMirror".to_string());
        }
        // TODO: Use BackendTLSPolicy to determine TLS instead of port heuristic.
        let scheme = if port == 443 { "https" } else { "http" };

        // namespace is always Some here: filled in from default_namespace at construction time.
        let ns = self.config.backend_ref.namespace.as_deref().unwrap_or_default();
        let service_key = format!("{}/{}", ns, self.config.backend_ref.name);

        // Use the gateway's standard backend resolution and selector cache.
        let section = service_section_key_from_port(Some(port))
            .map_err(|_| "mirror backend_ref.port is missing or invalid".to_string())?;
        if let Ok(endpoint) = select_service_endpoint(&service_key, section, &None, None, "", None) {
            let addr = endpoint.backend.inner().addr.clone();
            if validate_endpoint_backend_addr(&addr, allow_loopback_upstream).is_err() {
                return Err("mirror backend resolved to loopback/link-local/metadata address (rejected)".to_string());
            }
            let target = format!("{}", addr);
            return Ok((target.clone(), format!("{scheme}://{target}")));
        }

        Err("mirror backend has no selectable endpoint".to_string())
    }

    fn build_headers(&self, session: &dyn PluginSession, target: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let connection_value = session.header_str("connection").unwrap_or_default();
        let dynamic_listed = connection_listed_hop_by_hop(connection_value);
        for (name, value) in session.iter_headers() {
            if is_hop_by_hop_or_listed(name, &dynamic_listed)
                || name.eq_ignore_ascii_case("expect")
                || name.eq_ignore_ascii_case("host")
            {
                continue;
            }
            // Preserve the owned accessor's behavior: a header whose value is not
            // valid UTF-8 is skipped entirely, never forwarded as raw bytes.
            let Ok(value) = std::str::from_utf8(value) else {
                continue;
            };
            if let (Ok(hn), Ok(hv)) = (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(value)) {
                headers.append(hn, hv);
            }
        }

        if let Ok(host) = HeaderValue::from_str(target) {
            headers.insert("host", host);
        }
        headers.insert("x-mirror", HeaderValue::from_static("true"));
        headers
    }
}

#[async_trait]
impl RequestFilter for RequestMirrorPlugin {
    fn name(&self) -> &str {
        "RequestMirror"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        if let Some(reason) = self.skip_reason() {
            log.push(reason);
            return PluginRunningResult::GoodNext;
        }

        let permit = match self.semaphore.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                log.push("mirror skipped concurrency");
                return PluginRunningResult::GoodNext;
            }
        };

        let (target, base_url) = match self.resolve_target(session.ctx().allow_loopback_upstream) {
            Ok(v) => v,
            Err(_e) => {
                log.push_err("mirror resolve error");
                return PluginRunningResult::GoodNext;
            }
        };

        let path = session.get_path();
        let query = session.get_query();
        let url = if let Some(q) = query {
            format!("{base_url}{path}?{q}")
        } else {
            format!("{base_url}{path}")
        };

        let method = match reqwest::Method::from_bytes(session.get_method().as_bytes()) {
            Ok(m) => m,
            Err(_) => {
                log.push_err("mirror invalid method");
                return PluginRunningResult::GoodNext;
            }
        };
        let has_committed_body = session.ctx().request_body.has_cached_body();

        let committed_body = if has_committed_body {
            match session.read_request_body(None).await {
                Ok(body) => Some(body),
                Err(_) => {
                    log.push_err("mirror body snapshot error");
                    return PluginRunningResult::GoodNext;
                }
            }
        } else {
            None
        };

        let mut headers = self.build_headers(session, &target);
        if committed_body.is_some() {
            // This mirror hop owns framing for the immutable snapshot. The
            // downstream Content-Length may describe V0 rather than the
            // committed version, and reqwest derives the correct length from
            // Body::from(Bytes).
            headers.remove(reqwest::header::CONTENT_LENGTH);
            headers.remove(reqwest::header::TRANSFER_ENCODING);
        }
        let x_trace_id = session
            .ctx()
            .request_info
            .x_trace_id
            .clone()
            .unwrap_or_else(|| "-".to_string());

        let cfg = self.mirror_config();
        let (body_tx, body_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(cfg.max_buffered_chunks);
        let (cancellation, cancel_rx) = MirrorCancellation::new();
        let abandon_reason = cancellation.reason_handle();

        let channel_full_timeout_ms = self.tuning.channel_full_timeout_ms;
        let uses_committed_snapshot = committed_body.is_some();
        let client = get_mirror_http_client().clone();

        let writer_handle = tokio::spawn(async move {
            let _permit = permit;
            let begin = Instant::now();
            let total_timeout = cfg.connect_timeout + cfg.write_timeout;

            // Arc<AtomicU64> instead of Rc<Cell<u64>>: tokio::spawn requires Send futures,
            // so Rc is not usable here even though the closure runs in a single task.
            let bytes_sent = Arc::new(AtomicU64::new(
                committed_body.as_ref().map_or(0, |body| body.len() as u64),
            ));
            let chunks_sent = Arc::new(AtomicU64::new(u64::from(
                committed_body.as_ref().is_some_and(|body| !body.is_empty()),
            )));

            let bytes_ref = bytes_sent.clone();
            let chunks_ref = chunks_sent.clone();
            let tracked_stream = ReceiverStream::new(body_rx).map(move |chunk_result| {
                if let Ok(ref data) = chunk_result {
                    bytes_ref.fetch_add(data.len() as u64, Ordering::Relaxed);
                    chunks_ref.fetch_add(1, Ordering::Relaxed);
                }
                chunk_result
            });

            let mirror_body = match committed_body {
                Some(body) => reqwest::Body::from(body),
                None => reqwest::Body::wrap_stream(tracked_stream),
            };
            let send_result = tokio::time::timeout(total_timeout, async {
                client
                    .request(method, &url)
                    .headers(headers)
                    .body(mirror_body)
                    .send()
                    .await
            });
            let send_result = wait_for_send_or_cancel(send_result, cancel_rx).await;

            let (status, error) = match mirror_abandon_status(&abandon_reason) {
                Some(status) => (status, None),
                None => match send_result {
                    Some(Ok(Ok(resp))) => {
                        // The mirrored response is intentionally irrelevant to the
                        // main request. Dropping it after the headers avoids both
                        // unbounded aggregation and letting an infinite response
                        // retain the mirror concurrency permit.
                        drop(resp);
                        ("ok", None)
                    }
                    Some(Ok(Err(e))) => {
                        if e.is_connect() {
                            ("connect_err", Some("connection failed".to_string()))
                        } else if e.is_timeout() {
                            ("connect_timeout", None)
                        } else if e.is_body() {
                            ("write_err", Some("body stream error".to_string()))
                        } else {
                            ("write_err", Some("send failed".to_string()))
                        }
                    }
                    Some(Err(_)) => ("timeout", None),
                    None => ("body_abandoned", None),
                },
            };

            if cfg.mirror_log {
                let entry = new_entry(
                    x_trace_id,
                    target,
                    status,
                    Instant::now().saturating_duration_since(begin).as_millis() as u64,
                    bytes_sent.load(Ordering::Relaxed),
                    chunks_sent.load(Ordering::Relaxed),
                    error,
                );
                emit_mirror_log(entry).await;
            }

            record_mirror_metric(status);
        });

        // Only a transport-level END_STREAM on the request headers proves that
        // no body bytes can follow. `declares_body()` is intentionally not used:
        // it infers from Content-Length, which an H2 client can contradict by
        // sending DATA after `Content-Length: 0`. Arming the cancellation guard
        // when headers already ended the stream would make a later request-stage
        // rejection abort a mirror whose empty body is already complete. `false`
        // or `None` remains conservative and keeps the armed streaming path.
        let body_ended_at_headers = session.request_headers_end_stream() == Some(true);
        if uses_committed_snapshot || body_ended_at_headers {
            // Dropping a JoinHandle detaches the task. No request-body-filter
            // state is installed: for the committed snapshot the task owns an
            // immutable body and never consumes Pingora's replay cursor, and
            // for a headers-ended request there is nothing left to tee. Dropping
            // `body_tx` ends the writer's stream so reqwest closes the chunked
            // body cleanly, and `complete()` disarms the fail-closed guard so
            // the mirror is not reported as abandoned.
            drop(body_tx);
            cancellation.complete();
            drop(writer_handle);
        } else {
            if !session.set_mirror_state(MirrorState::Streaming {
                body_tx,
                writer_handle,
                cancellation,
                channel_full_timeout_ms,
            }) {
                // Dropping the rejected armed state cancels its detached writer
                // as BodyAbandoned; no body tap is installed after policy freeze.
                log.push_err("mirror body stage frozen");
                return PluginRunningResult::GoodNext;
            }
        }
        log.push("mirror sent");
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod arming_tests {
    use super::*;
    use crate::backends::get_roundrobin_store;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
    use crate::EdgionHttpContext;
    use edgion_resources::constants::labels::k8s::SERVICE_NAME;
    use edgion_resources::resources::http_route::BackendObjectReference;
    use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort, EndpointSlice};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::{BTreeMap, HashMap};

    const TEST_NS: &str = "mirror-arming-test";
    const TEST_SERVICE: &str = "mirror-arming-backend";
    // TEST-NET-1 (RFC 5737): routable-looking but never reachable, so the
    // detached writer task cannot connect to anything real.
    const TEST_ADDR: &str = "192.0.2.10";
    const TEST_PORT: i32 = 8080;

    /// Register a single-address EndpointSlice in the process-wide roundrobin
    /// store (additive `update_data_only`, so other tests sharing the singleton
    /// are undisturbed) so `resolve_target` finds a selectable endpoint.
    fn insert_test_endpoint_slice() {
        let mut labels = BTreeMap::new();
        labels.insert(SERVICE_NAME.to_string(), TEST_SERVICE.to_string());
        let slice = EndpointSlice {
            metadata: ObjectMeta {
                name: Some(format!("{TEST_SERVICE}-slice")),
                namespace: Some(TEST_NS.to_string()),
                labels: Some(labels),
                ..Default::default()
            },
            address_type: "IPv4".to_string(),
            endpoints: vec![Endpoint {
                addresses: vec![TEST_ADDR.to_string()],
                conditions: Some(EndpointConditions {
                    ready: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ports: Some(vec![EndpointPort {
                port: Some(TEST_PORT),
                ..Default::default()
            }]),
        };
        let mut add = HashMap::new();
        add.insert(format!("{TEST_NS}/{TEST_SERVICE}-slice"), slice);
        get_roundrobin_store().update_data_only(add, HashMap::new(), &Default::default());
    }

    fn test_plugin_with_tuning(mut tuning: MirrorTuning) -> RequestMirrorPlugin {
        // The mirror access log needs process-wide log infrastructure that a
        // unit test does not bootstrap.
        tuning.mirror_log = false;
        RequestMirrorPlugin::new(
            HTTPRequestMirrorFilter {
                backend_ref: BackendObjectReference {
                    group: String::new(),
                    kind: None,
                    name: TEST_SERVICE.to_string(),
                    namespace: Some(TEST_NS.to_string()),
                    port: Some(TEST_PORT),
                },
                fraction: None,
                percent: None,
            },
            tuning,
            TEST_NS.to_string(),
        )
    }

    fn test_plugin() -> RequestMirrorPlugin {
        test_plugin_with_tuning(MirrorTuning::default())
    }

    #[test]
    fn different_plugin_generations_share_the_process_mirror_client() {
        let before = get_mirror_http_client();

        let mut first_tuning = MirrorTuning::default();
        first_tuning.connect_timeout_ms = 11;
        first_tuning.write_timeout_ms = 17;
        let _first = test_plugin_with_tuning(first_tuning);
        let after_first = get_mirror_http_client();

        let mut second_tuning = MirrorTuning::default();
        second_tuning.connect_timeout_ms = 23;
        second_tuning.write_timeout_ms = 29;
        let _second = test_plugin_with_tuning(second_tuning);
        let after_second = get_mirror_http_client();

        assert!(std::ptr::eq(before, after_first));
        assert!(std::ptr::eq(before, after_second));
    }

    fn test_session(headers_end_stream: Option<bool>) -> MockPluginSession {
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session
            .expect_request_headers_end_stream()
            .return_const(headers_end_stream);
        session.expect_declares_body().never();
        session.expect_get_path().return_const("/mirror".to_string());
        session.expect_get_query().return_const(None);
        session.expect_get_method().return_const("POST".to_string());
        setup_header_mocks(&mut session, &[]);
        session
    }

    #[tokio::test]
    async fn headers_end_stream_does_not_install_armed_streaming_state() {
        insert_test_endpoint_slice();
        let plugin = test_plugin();
        let mut session = test_session(Some(true));
        // A transport-level header END_STREAM proves the empty body complete,
        // so no armed mirror state is installed.
        session.expect_set_mirror_state().never();
        let mut log = PluginLog::new("RequestMirror");

        let result = plugin.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn headers_without_end_stream_keep_armed_streaming_state() {
        insert_test_endpoint_slice();
        let plugin = test_plugin();
        let mut session = test_session(Some(false));
        // This is the H2 `Content-Length: 0` without END_STREAM shape: header
        // declarations cannot replace the transport fact. It also proves the
        // fixture resolves a real mirror target; otherwise the positive test
        // above could pass because of an early mirror-resolution return.
        session.expect_set_mirror_state().times(1).returning(|state| {
            assert!(matches!(state, MirrorState::Streaming { .. }));
            true
        });
        let mut log = PluginLog::new("RequestMirror");

        let result = plugin.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn unknown_headers_end_stream_keeps_armed_streaming_state() {
        insert_test_endpoint_slice();
        let plugin = test_plugin();
        let mut session = test_session(None);
        // `None` means the session cannot provide the transport fact: stay
        // conservative.
        session.expect_set_mirror_state().times(1).returning(|state| {
            assert!(matches!(state, MirrorState::Streaming { .. }));
            true
        });
        let mut log = PluginLog::new("RequestMirror");

        let result = plugin.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn frozen_body_stage_refuses_streaming_mirror_install() {
        insert_test_endpoint_slice();
        let plugin = test_plugin();
        let mut session = test_session(Some(false));
        session.expect_set_mirror_state().times(1).return_once(|state| {
            assert!(matches!(state, MirrorState::Streaming { .. }));
            false
        });
        let mut log = PluginLog::new("RequestMirror");

        let result = plugin.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;

    #[tokio::test]
    async fn explicit_cancellation_wins_when_result_is_also_ready() {
        let (cancel_tx, cancel_rx) = oneshot::channel();
        cancel_tx.send(()).unwrap();

        assert_eq!(wait_for_send_or_cancel(async { 7_u8 }, cancel_rx).await, None);
    }

    #[tokio::test]
    async fn clean_completion_waits_for_send_result() {
        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
        drop(cancel_tx);

        assert_eq!(wait_for_send_or_cancel(async { 7_u8 }, cancel_rx).await, Some(7));
    }

    #[test]
    fn cancellation_reason_is_recorded_before_notifying_task() {
        let (cancellation, mut cancel_rx) = MirrorCancellation::new();
        let reason = cancellation.reason_handle();
        cancellation.cancel(MirrorAbandonReason::ChannelFull);

        assert_eq!(
            MirrorCancellation::load_reason(&reason),
            Some(MirrorAbandonReason::ChannelFull)
        );
        assert!(cancel_rx.try_recv().is_ok());
    }

    #[test]
    fn dropping_armed_guard_fails_closed_as_body_abandoned() {
        let (cancellation, mut cancel_rx) = MirrorCancellation::new();
        let reason = cancellation.reason_handle();
        drop(cancellation);

        assert_eq!(
            MirrorCancellation::load_reason(&reason),
            Some(MirrorAbandonReason::BodyAbandoned)
        );
        assert_eq!(mirror_abandon_status(&reason), Some("body_abandoned"));
        assert!(cancel_rx.try_recv().is_ok());
    }
}
