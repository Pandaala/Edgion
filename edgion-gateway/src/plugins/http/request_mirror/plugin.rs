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

use crate::backends::{
    select_service_endpoint, service_section_key_from_port, validate_endpoint_backend_addr, ServiceDegradePolicy,
};
use crate::ctx::{MirrorAbandonReason, MirrorCancellation};
use crate::observe::metrics::record_mirror_metric;
#[cfg(not(test))]
use crate::plugins::http::common::http_client::get_mirror_http_client;
use crate::plugins::http::common::http_client::{connection_listed_hop_by_hop, is_hop_by_hop_or_listed};
use crate::plugins::runtime::{PluginLog, PluginSession, PluginSessionHeaderExt, RequestFilter};
use crate::{MirrorConfig, MirrorState};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::http_route::{HTTPRequestMirrorFilter, MirrorTuning};

use super::budget::{global_budget, MirrorBudget};
use super::mirror_log::{emit_mirror_log, new_entry};
use super::MirrorBodyBudget;

pub struct RequestMirrorPlugin {
    config: HTTPRequestMirrorFilter,
    tuning: MirrorTuning,
    semaphore: Arc<Semaphore>,
    #[cfg(test)]
    writer_events: Option<mpsc::UnboundedSender<arming_tests::WriterEvent>>,
    budget: Arc<MirrorBudget>,
    max_buffered_bytes: usize,
}

struct PreparedSnapshot {
    // Release memory before returning the admission charge.
    reader: Box<dyn crate::plugins::runtime::traits::session::RequestBodySnapshotReader>,
    _memory_permit: tokio::sync::OwnedSemaphorePermit,
}

fn prepare_snapshot(
    mut reader: Box<dyn crate::plugins::runtime::traits::session::RequestBodySnapshotReader>,
    budget: &MirrorBudget,
) -> Result<PreparedSnapshot, &'static str> {
    let bytes = reader.memory_bytes_to_isolate().ok_or("mirror snapshot unavailable")?;
    let permit = budget
        .try_reserve_snapshot(bytes)
        .ok_or("mirror skipped buffer limit")?;
    // Isolation is synchronous: never extend an unaccounted backing allocation
    // across an await or retain it in the detached task. Disk segments stay on disk.
    reader.isolate_memory().map_err(|_| "mirror snapshot isolation error")?;
    Ok(PreparedSnapshot {
        reader,
        _memory_permit: permit,
    })
}

fn snapshot_stream(
    body: PreparedSnapshot,
    budget: MirrorBodyBudget,
    abandon_reason: Arc<std::sync::atomic::AtomicU8>,
    bytes_sent: Arc<AtomicU64>,
    chunks_sent: Arc<AtomicU64>,
) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send {
    futures::stream::try_unfold(body, move |mut body| {
        let budget = budget.clone();
        let abandon_reason = Arc::clone(&abandon_reason);
        let bytes_sent = Arc::clone(&bytes_sent);
        let chunks_sent = Arc::clone(&chunks_sent);
        async move {
            // The canonical storage lane bounds transient disk-read scratch.
            // Only retained send payload and isolated snapshot memory belong to
            // this budget; never materialize an entire disk-backed request.
            let data = body
                .reader
                .next_chunk()
                .await
                .map_err(|_| std::io::Error::other("mirror snapshot read failed"))?;
            let Some(data) = data else {
                return Ok(None);
            };
            let Some(owned) = budget.try_copy(&data) else {
                abandon_reason.store(MirrorAbandonReason::BufferLimit as u8, Ordering::Release);
                return Err(std::io::Error::other("mirror body budget exhausted"));
            };
            bytes_sent.fetch_add(owned.len() as u64, Ordering::Relaxed);
            chunks_sent.fetch_add(1, Ordering::Relaxed);
            Ok(Some((owned, body)))
        }
    })
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
        Some(MirrorAbandonReason::BufferLimit) => Some("buffer_limit"),
        None => None,
    }
}

impl RequestMirrorPlugin {
    pub fn new(config: HTTPRequestMirrorFilter, tuning: MirrorTuning, default_namespace: String) -> Self {
        let mut cfg = config;
        let mut tuning = tuning;
        let limits = tuning.normalize_limits();
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
            budget: global_budget(),
            max_buffered_bytes: limits.max_buffered_bytes,
            config: cfg,
            tuning,
            #[cfg(test)]
            writer_events: None,
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
        if let Ok(endpoint) =
            select_service_endpoint(&service_key, section, &None, ServiceDegradePolicy::Live, None, "", None)
        {
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
        if self.config.backend_ref.ref_denied.is_some() {
            log.push_err("mirror ref denied");
            return PluginRunningResult::GoodNext;
        }
        // Never authorize one resource kind and dial a same-named Service.
        if !self.config.backend_ref.is_service() {
            log.push_err("mirror invalid kind");
            return PluginRunningResult::GoodNext;
        }
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

        let Some(process_permit) = self.budget.try_acquire_task() else {
            log.push("mirror skipped process concurrency");
            return PluginRunningResult::GoodNext;
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
            let Ok(reader) = session.request_body_snapshot_reader(false).await else {
                log.push_err("mirror body snapshot error");
                return PluginRunningResult::GoodNext;
            };
            match prepare_snapshot(reader, &self.budget) {
                Ok(body) => Some(body),
                Err(reason) => {
                    log.push(reason);
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
            // committed version. The independent snapshot owns its framing.
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
        let body_budget = self.budget.body_budget(self.max_buffered_bytes);
        let (body_tx, body_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(cfg.max_buffered_chunks);
        let (cancellation, cancel_rx) = MirrorCancellation::new();
        let abandon_reason = cancellation.reason_handle();

        let channel_full_timeout_ms = self.tuning.channel_full_timeout_ms;
        let uses_committed_snapshot = committed_body.is_some();
        let snapshot_body_budget = body_budget.clone();
        #[cfg(not(test))]
        let client = get_mirror_http_client().clone();
        #[cfg(test)]
        let writer_events = self
            .writer_events
            .clone()
            .expect("arming tests must observe the writer");

        let writer_handle = tokio::spawn(async move {
            let _permit = permit;
            let _process_permit = process_permit;
            let begin = Instant::now();
            let total_timeout = cfg.connect_timeout + cfg.write_timeout;

            // Arc<AtomicU64> instead of Rc<Cell<u64>>: tokio::spawn requires Send futures,
            // so Rc is not usable here even though the closure runs in a single task.
            let bytes_sent = Arc::new(AtomicU64::new(0));
            let chunks_sent = Arc::new(AtomicU64::new(0));

            let bytes_ref = bytes_sent.clone();
            let chunks_ref = chunks_sent.clone();
            let tracked_stream = ReceiverStream::new(body_rx).map(move |chunk_result| {
                if let Ok(ref data) = chunk_result {
                    bytes_ref.fetch_add(data.len() as u64, Ordering::Relaxed);
                    chunks_ref.fetch_add(1, Ordering::Relaxed);
                }
                chunk_result
            });

            let mirror_stream = match committed_body {
                Some(body) => futures::future::Either::Left(snapshot_stream(
                    body,
                    snapshot_body_budget,
                    Arc::clone(&abandon_reason),
                    Arc::clone(&bytes_sent),
                    Arc::clone(&chunks_sent),
                )),
                None => futures::future::Either::Right(tracked_stream),
            };
            #[cfg(not(test))]
            let send = async {
                // Discard the response after headers without aggregating it.
                client
                    .request(method, &url)
                    .headers(headers)
                    .body(reqwest::Body::wrap_stream(mirror_stream))
                    .send()
                    .await
                    .map(drop)
            };
            // Unit tests exercise the same arming, counters, cancellation and
            // outcome path, but cannot compile a DNS/socket send into this writer.
            #[cfg(test)]
            let send = arming_tests::consume_without_network(
                &writer_events,
                method,
                url,
                headers,
                uses_committed_snapshot,
                mirror_stream,
            );
            let send_result = tokio::time::timeout(total_timeout, send);
            let send_result = wait_for_send_or_cancel(send_result, cancel_rx).await;

            let (status, error) = match mirror_abandon_status(&abandon_reason) {
                Some(status) => (status, None),
                None => match send_result {
                    Some(Ok(Ok(()))) => ("ok", None),
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
            #[cfg(test)]
            {
                drop(_permit);
                writer_events
                    .send(arming_tests::WriterEvent::Finished {
                        status,
                        bytes: bytes_sent.load(Ordering::Relaxed),
                        chunks: chunks_sent.load(Ordering::Relaxed),
                    })
                    .unwrap();
            }
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
                body_budget,
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
    use crate::plugins::http::common::http_client::get_mirror_http_client;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
    use crate::EdgionHttpContext;
    use edgion_resources::constants::labels::k8s::SERVICE_NAME;
    use edgion_resources::resources::http_route::BackendObjectReference;
    use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort, EndpointSlice};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use pingora_core::protocols::http::RequestBodyBuffer;
    use std::collections::{BTreeMap, HashMap};

    const TEST_NS: &str = "mirror-arming-test";
    const TEST_SERVICE: &str = "mirror-arming-backend";
    // TEST-NET-1 is only endpoint-selection data. The test writer has no transport.
    const TEST_ADDR: &str = "192.0.2.10";
    const TEST_PORT: i32 = 8080;

    pub(super) enum WriterEvent {
        Prepared {
            method: reqwest::Method,
            url: String,
            headers: HeaderMap,
            snapshot: Option<Bytes>,
        },
        Chunk(Bytes),
        Finished {
            status: &'static str,
            bytes: u64,
            chunks: u64,
        },
    }

    pub(super) async fn consume_without_network(
        events: &mpsc::UnboundedSender<WriterEvent>,
        method: reqwest::Method,
        url: String,
        headers: HeaderMap,
        uses_snapshot: bool,
        stream: impl tokio_stream::Stream<Item = Result<Bytes, std::io::Error>>,
    ) -> Result<(), reqwest::Error> {
        tokio::pin!(stream);
        let snapshot = if uses_snapshot {
            let mut data = Vec::new();
            while let Some(chunk) = stream.next().await {
                data.extend_from_slice(&chunk.unwrap());
            }
            Some(Bytes::from(data))
        } else {
            None
        };
        events
            .send(WriterEvent::Prepared {
                method,
                url,
                headers,
                snapshot,
            })
            .unwrap();
        if !uses_snapshot {
            while let Some(chunk) = stream.next().await {
                events.send(WriterEvent::Chunk(chunk.unwrap())).unwrap();
            }
        }
        Ok(())
    }

    fn observe_writer(plugin: &mut RequestMirrorPlugin) -> mpsc::UnboundedReceiver<WriterEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        plugin.writer_events = Some(tx);
        rx
    }

    async fn next_event(events: &mut mpsc::UnboundedReceiver<WriterEvent>) -> WriterEvent {
        tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap()
    }

    async fn assert_prepared(events: &mut mpsc::UnboundedReceiver<WriterEvent>, snapshot: Option<Bytes>) {
        let WriterEvent::Prepared {
            method,
            url,
            headers,
            snapshot: actual,
        } = next_event(events).await
        else {
            panic!("expected prepared mirror request");
        };
        assert_eq!(method, reqwest::Method::POST);
        assert_eq!(url, format!("http://{TEST_ADDR}:{TEST_PORT}/mirror"));
        assert_eq!(headers["host"], format!("{TEST_ADDR}:{TEST_PORT}"));
        assert_eq!(headers["x-mirror"], "true");
        assert_eq!(actual, snapshot);
        if snapshot.is_some() {
            assert!(!headers.contains_key("content-length"));
            assert!(!headers.contains_key("transfer-encoding"));
        }
    }

    async fn assert_finished(
        events: &mut mpsc::UnboundedReceiver<WriterEvent>,
        plugin: &RequestMirrorPlugin,
        expected: (&str, u64, u64),
    ) {
        let WriterEvent::Finished { status, bytes, chunks } = next_event(events).await else {
            panic!("expected completed mirror writer");
        };
        assert_eq!((status, bytes, chunks), expected);
        assert_eq!(plugin.semaphore.available_permits(), plugin.tuning.max_concurrent);
        assert!(events.try_recv().is_err());
    }

    /// Publish the shared immutable fixture once: concurrent replacements of
    /// the same service can transiently invalidate endpoint selection.
    fn insert_test_endpoint_slice() {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(publish_test_endpoint_slice);
    }

    fn publish_test_endpoint_slice() {
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
                    ref_denied: None,
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
        test_session_with_context(headers_end_stream, EdgionHttpContext::new(), &[])
    }

    fn test_session_with_context(
        headers_end_stream: Option<bool>,
        ctx: EdgionHttpContext,
        headers: &[(&str, &str)],
    ) -> MockPluginSession {
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx);
        session
            .expect_request_headers_end_stream()
            .return_const(headers_end_stream);
        session.expect_declares_body().never();
        session.expect_get_path().return_const("/mirror".to_string());
        session.expect_get_query().return_const(None);
        session.expect_get_method().return_const("POST".to_string());
        setup_header_mocks(&mut session, headers);
        session
    }

    #[tokio::test]
    async fn disabled_sampling_and_saturated_concurrency_do_not_start_writer() {
        for disabled in [true, false] {
            let mut plugin = test_plugin();
            let mut events = observe_writer(&mut plugin);
            let held = if disabled {
                plugin.config.percent = Some(0);
                None
            } else {
                Some(
                    plugin
                        .semaphore
                        .clone()
                        .try_acquire_many_owned(plugin.tuning.max_concurrent as u32)
                        .unwrap(),
                )
            };
            // Any endpoint/session access would fail an unmet mock expectation.
            let mut session = MockPluginSession::new();
            let mut log = PluginLog::new("RequestMirror");
            assert_eq!(
                plugin.run_request(&mut session, &mut log).await,
                PluginRunningResult::GoodNext
            );
            assert_eq!(
                log.render_for_test(),
                if disabled {
                    "mirror disabled"
                } else {
                    "mirror skipped concurrency"
                }
            );
            assert!(events.try_recv().is_err());
            drop(held);
            assert_eq!(plugin.semaphore.available_permits(), plugin.tuning.max_concurrent);
        }
    }

    #[tokio::test]
    async fn denied_or_wrong_kind_mirror_never_arms_or_uses_session() {
        for (denied, group, kind, diagnostic) in [
            (true, "", "Service", "mirror ref denied"),
            (false, "example.com", "Service", "mirror invalid kind"),
            (false, "", "Secret", "mirror invalid kind"),
        ] {
            let mut plugin = test_plugin();
            plugin.config.backend_ref.group = group.to_string();
            plugin.config.backend_ref.kind = Some(kind.to_string());
            plugin.config.backend_ref.ref_denied = denied.then(|| edgion_resources::resources::common::RefDenied {
                target_namespace: TEST_NS.to_string(),
                target_name: TEST_SERVICE.to_string(),
                reason: None,
            });
            let mut events = observe_writer(&mut plugin);
            // No session expectation is installed: even reading the request
            // context or preparing its body before rejecting fails this test.
            let mut session = MockPluginSession::new();
            let mut log = PluginLog::new("RequestMirror");
            assert_eq!(
                plugin.run_request(&mut session, &mut log).await,
                PluginRunningResult::GoodNext
            );
            assert!(log.render_for_test().contains(diagnostic));
            assert!(events.try_recv().is_err());
            assert_eq!(plugin.semaphore.available_permits(), plugin.tuning.max_concurrent);
        }
    }

    #[tokio::test]
    async fn committed_snapshot_has_independent_body_and_framing() {
        insert_test_endpoint_slice();
        let mut plugin = test_plugin();
        let mut events = observe_writer(&mut plugin);
        let (mut storage, view) = crate::request_body::BodyStorage::new(1024, 1024);
        storage.write(&Bytes::from_static(b"original")).await.unwrap();
        storage.finish().await.unwrap();
        view.overwrite_body(Bytes::from_static(b"committed")).unwrap();
        let mut ctx = EdgionHttpContext::new();
        ctx.request_body.install(view.clone());
        let mut session = test_session_with_context(
            Some(false),
            ctx,
            &[("content-length", "8"), ("transfer-encoding", "chunked")],
        );
        let snapshot = view.snapshot_reader().unwrap();
        session
            .expect_request_body_snapshot_reader()
            .withf(|original| !*original)
            .times(1)
            .return_once(move |_| Ok(snapshot));
        session.expect_set_mirror_state().never();
        let mut log = PluginLog::new("RequestMirror");

        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert_eq!(log.render_for_test(), "mirror sent");
        view.overwrite_body(Bytes::from_static(b"later")).unwrap();
        assert_prepared(&mut events, Some(Bytes::from_static(b"committed"))).await;
        assert_finished(&mut events, &plugin, ("ok", 9, 1)).await;
        assert_eq!(view.read_all().await.unwrap(), "later");
    }

    #[tokio::test]
    async fn headers_end_stream_does_not_install_armed_streaming_state() {
        insert_test_endpoint_slice();
        let mut plugin = test_plugin();
        let mut events = observe_writer(&mut plugin);
        let mut session = test_session(Some(true));
        // A transport-level header END_STREAM proves the empty body complete,
        // so no armed mirror state is installed.
        session.expect_set_mirror_state().never();
        let mut log = PluginLog::new("RequestMirror");

        let result = plugin.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(log.render_for_test(), "mirror sent");
        assert_prepared(&mut events, None).await;
        assert_finished(&mut events, &plugin, ("ok", 0, 0)).await;
    }

    #[tokio::test]
    async fn headers_without_end_stream_keep_armed_streaming_state() {
        insert_test_endpoint_slice();
        let mut plugin = test_plugin();
        let mut events = observe_writer(&mut plugin);
        let mut session = test_session(Some(false));
        // This is the H2 `Content-Length: 0` without END_STREAM shape: header
        // declarations cannot replace the transport fact. It also proves the
        // fixture resolves a real mirror target; otherwise the positive test
        // above could pass because of an early mirror-resolution return.
        let (state_tx, state_rx) = oneshot::channel();
        session.expect_set_mirror_state().times(1).return_once(|state| {
            assert!(matches!(state, MirrorState::Streaming { .. }));
            assert!(state_tx.send(state).is_ok());
            true
        });
        let mut log = PluginLog::new("RequestMirror");

        let result = plugin.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(log.render_for_test(), "mirror sent");
        assert_prepared(&mut events, None).await;
        assert_eq!(plugin.semaphore.available_permits(), plugin.tuning.max_concurrent - 1);
        let MirrorState::Streaming {
            body_tx,
            writer_handle,
            cancellation,
            ..
        } = state_rx.await.unwrap()
        else {
            panic!("expected streaming mirror");
        };
        body_tx.send(Ok(Bytes::from_static(b"streamed body"))).await.unwrap();
        let WriterEvent::Chunk(body) = next_event(&mut events).await else {
            panic!("expected streamed body chunk");
        };
        assert_eq!(body, "streamed body");
        cancellation.complete();
        drop(body_tx);
        writer_handle.await.unwrap();
        assert_finished(&mut events, &plugin, ("ok", 13, 1)).await;
    }

    #[tokio::test]
    async fn unknown_headers_end_stream_keeps_armed_streaming_state() {
        insert_test_endpoint_slice();
        let mut plugin = test_plugin();
        let mut events = observe_writer(&mut plugin);
        let mut session = test_session(None);
        // `None` means the session cannot provide the transport fact: stay
        // conservative.
        let (state_tx, state_rx) = oneshot::channel();
        session.expect_set_mirror_state().times(1).return_once(|state| {
            assert!(matches!(state, MirrorState::Streaming { .. }));
            assert!(state_tx.send(state).is_ok());
            true
        });
        let mut log = PluginLog::new("RequestMirror");

        let result = plugin.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(log.render_for_test(), "mirror sent");
        assert_prepared(&mut events, None).await;
        drop(state_rx.await.unwrap());
        assert_finished(&mut events, &plugin, ("body_abandoned", 0, 0)).await;
    }

    #[tokio::test]
    async fn frozen_body_stage_refuses_streaming_mirror_install() {
        insert_test_endpoint_slice();
        let mut plugin = test_plugin();
        let mut events = observe_writer(&mut plugin);
        let mut session = test_session(Some(false));
        session.expect_set_mirror_state().times(1).return_once(|state| {
            assert!(matches!(state, MirrorState::Streaming { .. }));
            false
        });
        let mut log = PluginLog::new("RequestMirror");

        let result = plugin.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(log.render_for_test(), "err: mirror body stage frozen");
        // Cancellation is already ready and wins without polling the body/send future.
        assert_finished(&mut events, &plugin, ("body_abandoned", 0, 0)).await;
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

// Budget regression tests for this module live in `tests/plugin_test.rs`.
#[cfg(test)]
#[path = "tests/plugin_test.rs"]
mod budget_tests;
