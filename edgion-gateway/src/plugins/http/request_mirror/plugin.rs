use edgion_resources::resources::refs::route_fields::EffectiveMirrorField;
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
    target: Result<CompiledMirrorTarget, &'static str>,
    tuning: MirrorTuning,
    semaphore: Arc<Semaphore>,
    #[cfg(test)]
    writer_events: Option<mpsc::UnboundedSender<arming_tests::WriterEvent>>,
    budget: Arc<MirrorBudget>,
    max_buffered_bytes: usize,
}

struct CompiledMirrorTarget {
    service_key: String,
    section: crate::backends::discovery::ServiceSectionKey,
    scheme: &'static str,
}

struct PreparedSnapshot {
    // Release memory before returning the admission charge.
    reader: Box<dyn crate::request_body::RequestBodySnapshotReader>,
    _memory_permit: tokio::sync::OwnedSemaphorePermit,
}

fn prepare_snapshot(
    mut reader: Box<dyn crate::request_body::RequestBodySnapshotReader>,
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
    pub fn new(field: EffectiveMirrorField<'_>, tuning: MirrorTuning) -> Option<Self> {
        let cfg = field.config()?.clone();
        let target = if cfg.backend_ref.ref_denied.is_some() {
            Err("mirror ref denied")
        } else {
            match field.validated() {
                Ok(Some(reference)) => service_section_key_from_port(Some(reference.port()))
                    .map(|section| CompiledMirrorTarget {
                        service_key: format!(
                            "{}/{}",
                            reference.target().scope().namespace().unwrap_or_default(),
                            reference.target().name()
                        ),
                        section,
                        scheme: if reference.port() == 443 { "https" } else { "http" },
                    })
                    .map_err(|_| "mirror resolve error"),
                Err(edgion_resources::resources::refs::ReferenceError::UnsupportedTarget) => Err("mirror invalid kind"),
                _ => Err("mirror resolve error"),
            }
        };
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

        Some(Self {
            target,
            semaphore: Arc::new(Semaphore::new(tuning.max_concurrent)),
            budget: global_budget(),
            max_buffered_bytes: limits.max_buffered_bytes,
            config: cfg,
            tuning,
            #[cfg(test)]
            writer_events: None,
        })
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
        let target = self.target.as_ref().map_err(|reason| (*reason).to_string())?;
        let scheme = target.scheme;
        if let Ok(endpoint) = select_service_endpoint(
            &target.service_key,
            target.section.clone(),
            &None,
            ServiceDegradePolicy::Live,
            None,
            "",
            None,
        ) {
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
        if let Err(reason) = &self.target {
            log.push_err(reason);
            return PluginRunningResult::GoodNext;
        }
        if let Some(reason) = self.skip_reason() {
            log.push(reason);
            return PluginRunningResult::GoodNext;
        }

        let Ok(permit) = self.semaphore.clone().try_acquire_owned() else {
            log.push("mirror skipped concurrency");
            return PluginRunningResult::GoodNext;
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

        let Ok(method) = reqwest::Method::from_bytes(session.get_method().as_bytes()) else {
            log.push_err("mirror invalid method");
            return PluginRunningResult::GoodNext;
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
        let trace_id = session
            .ctx()
            .request_info
            .trace_id
            .standard_trace_id()
            .map(str::to_owned)
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
                    trace_id,
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
#[path = "../../../../tests/unit/plugins/http/request_mirror/plugin_test/arming_tests.rs"]
mod arming_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/request_mirror/plugin_test/cancellation_tests.rs"]
mod cancellation_tests;

// Budget regression tests for this module live in `tests/plugin_test.rs`.
#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/request_mirror/plugin_test/budget_tests.rs"]
mod budget_tests;
