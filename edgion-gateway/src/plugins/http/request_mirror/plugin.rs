use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use rand::Rng;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::sync::{mpsc, Semaphore};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use crate::backends::{resolve_target_port_key, select_roundrobin_backend_key, validate_endpoint_backend_addr};
use crate::observe::metrics::record_mirror_metric;
use crate::plugins::http::common::http_client::{
    build_pooled_outbound_client, connection_listed_hop_by_hop, is_hop_by_hop_or_listed,
};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::{MirrorConfig, MirrorState};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::http_route::{HTTPRequestMirrorFilter, MirrorTuning};

use super::mirror_log::{emit_mirror_log, new_entry};

pub struct RequestMirrorPlugin {
    config: HTTPRequestMirrorFilter,
    tuning: MirrorTuning,
    semaphore: Arc<Semaphore>,
    client: reqwest::Client,
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

        let connect_timeout = Duration::from_millis(tuning.connect_timeout_ms.max(1));
        let client =
            build_pooled_outbound_client(connect_timeout, None).expect("failed to build RequestMirror reqwest client");

        // If backendRef.namespace is absent, default to current route/plugin namespace.
        if cfg.backend_ref.namespace.is_none() {
            cfg.backend_ref.namespace = Some(default_namespace);
        }

        Self {
            semaphore: Arc::new(Semaphore::new(tuning.max_concurrent)),
            config: cfg,
            tuning,
            client,
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
        if let Some(backend) = select_roundrobin_backend_key(&service_key) {
            let mut addr = backend.inner().addr.clone();
            resolve_target_port_key(&mut addr, Some(port), &service_key);
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
        let connection_value = session.header_value("connection").unwrap_or_default();
        let dynamic_listed = connection_listed_hop_by_hop(&connection_value);
        for (name, value) in session.request_headers() {
            if is_hop_by_hop_or_listed(&name, &dynamic_listed)
                || name.eq_ignore_ascii_case("expect")
                || name.eq_ignore_ascii_case("host")
            {
                continue;
            }
            if let (Ok(hn), Ok(hv)) = (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(&value)) {
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
        #[cfg(feature = "body-buffering")]
        let has_committed_body = session.ctx().request_body.is_some();
        #[cfg(not(feature = "body-buffering"))]
        let has_committed_body = false;

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
        let client = self.client.clone();

        // Shared flag for channel-full detection.
        // request_body_filter sets this to true when it abandons the mirror due to a full channel.
        // The mirror task reads it at completion to distinguish "channel_full" from "write_err".
        let channel_full_flag = Arc::new(AtomicBool::new(false));
        let task_channel_full_flag = channel_full_flag.clone();

        let channel_full_timeout_ms = self.tuning.channel_full_timeout_ms;
        let uses_committed_snapshot = committed_body.is_some();

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
            })
            .await;

            let (status, error) = match send_result {
                Ok(Ok(resp)) => {
                    // The mirrored response is intentionally irrelevant to the
                    // main request. Dropping it after the headers avoids both
                    // unbounded aggregation and letting an infinite response
                    // retain the mirror concurrency permit.
                    drop(resp);
                    ("ok", None)
                }
                Ok(Err(e)) => {
                    // Check channel_full_flag first: it is set by request_body_filter when the
                    // mirror is abandoned because the body channel was full (either immediately or
                    // after the channel_full_timeout_ms window expired). In that case, reqwest sees
                    // the body stream end (body_tx dropped) and may return Ok or an error depending
                    // on whether the body was already partially flushed. We must check the flag
                    // *before* inspecting the error kind to avoid misreporting "write_err".
                    if task_channel_full_flag.load(Ordering::Relaxed) {
                        ("channel_full", None)
                    } else if e.is_connect() {
                        ("connect_err", Some("connection failed".to_string()))
                    } else if e.is_timeout() {
                        ("connect_timeout", None)
                    } else if e.is_body() {
                        ("write_err", Some("body stream error".to_string()))
                    } else {
                        ("write_err", Some("send failed".to_string()))
                    }
                }
                Err(_) => ("timeout", None),
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

        if uses_committed_snapshot {
            // Dropping a JoinHandle detaches the task. No request-body-filter
            // state is installed because the task owns an immutable body
            // snapshot and never consumes Pingora's replay cursor.
            drop(body_tx);
            drop(writer_handle);
        } else {
            session.set_mirror_state(MirrorState::Streaming {
                body_tx,
                writer_handle,
                channel_full_flag,
                channel_full_timeout_ms,
            });
        }
        log.push("mirror sent");
        PluginRunningResult::GoodNext
    }
}
