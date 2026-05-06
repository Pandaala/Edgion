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

use crate::core::gateway::backends::{
    resolve_target_port_key, select_roundrobin_backend_key, validate_endpoint_backend_addr, PortLookupSource,
};
use crate::core::gateway::observe::metrics::record_mirror_metric;
use crate::core::gateway::plugins::http::common::http_client::is_hop_by_hop;
use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::http_route::HTTPRequestMirrorFilter;
use crate::types::{MirrorConfig, MirrorState};

use super::mirror_log::{emit_mirror_log, new_entry};

pub struct RequestMirrorPlugin {
    config: HTTPRequestMirrorFilter,
    semaphore: Arc<Semaphore>,
    client: reqwest::Client,
}

impl RequestMirrorPlugin {
    pub fn new(config: HTTPRequestMirrorFilter, default_namespace: String) -> Self {
        let mut cfg = config;
        if cfg.max_buffered_chunks == 0 {
            cfg.max_buffered_chunks = 5;
        }
        if cfg.max_concurrent == 0 {
            cfg.max_concurrent = 1024;
        }
        // Hard cap: channel_full_timeout_ms must not exceed 1000ms.
        // Values above 1s would add unacceptable back-pressure to main request body processing.
        if cfg.channel_full_timeout_ms > 1000 {
            tracing::warn!(
                channel_full_timeout_ms = cfg.channel_full_timeout_ms,
                "channel_full_timeout_ms exceeds maximum (1000ms); clamping to 1000ms"
            );
            cfg.channel_full_timeout_ms = 1000;
        }

        let connect_timeout = Duration::from_millis(cfg.connect_timeout_ms.max(1));
        let client = reqwest::Client::builder()
            .pool_max_idle_per_host(32)
            .connect_timeout(connect_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("failed to build RequestMirror reqwest client");

        // If backendRef.namespace is absent, default to current route/plugin namespace.
        if cfg.backend_ref.namespace.is_none() {
            cfg.backend_ref.namespace = Some(default_namespace);
        }

        Self {
            semaphore: Arc::new(Semaphore::new(cfg.max_concurrent)),
            config: cfg,
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
        if let Some(pct) = self.config.percentage {
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
            connect_timeout: Duration::from_millis(self.config.connect_timeout_ms.max(1)),
            write_timeout: Duration::from_millis(self.config.write_timeout_ms.max(1)),
            max_buffered_chunks: self.config.max_buffered_chunks,
            mirror_log: self.config.mirror_log,
            // max_concurrent omitted: managed by self.semaphore, not needed inside the task.
        }
    }

    fn resolve_target(&self, allow_loopback_upstream: bool) -> Result<(String, String), String> {
        let port = self
            .config
            .backend_ref
            .port
            .ok_or_else(|| "backend_ref.port is required for RequestMirror".to_string())?;
        if port <= 0 {
            return Err("backend_ref.port must be > 0".to_string());
        }
        if self.config.backend_ref.name.is_empty() {
            return Err("backend_ref.name is required for RequestMirror".to_string());
        }
        // TODO(Phase 2): Use BackendTLSPolicy to determine TLS instead of port heuristic.
        let scheme = if port == 443 { "https" } else { "http" };

        // namespace is always Some here: filled in from default_namespace at construction time.
        let ns = self.config.backend_ref.namespace.as_deref().unwrap_or_default();
        let service_key = format!("{}/{}", ns, self.config.backend_ref.name);

        // Use the gateway's standard backend resolution which respects EndpointMode,
        // performs round-robin LB across endpoints, and honours health checks.
        if let Some(backend) = select_roundrobin_backend_key(&service_key) {
            let mut addr = backend.inner().addr.clone();
            resolve_target_port_key(&mut addr, Some(port), &service_key, PortLookupSource::Auto);
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
        for (name, value) in session.request_headers() {
            if is_hop_by_hop(&name) || name.eq_ignore_ascii_case("expect") || name.eq_ignore_ascii_case("host") {
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
        let headers = self.build_headers(session, &target);
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

        let channel_full_timeout_ms = self.config.channel_full_timeout_ms;

        let writer_handle = tokio::spawn(async move {
            let _permit = permit;
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

            let stream_body = reqwest::Body::wrap_stream(tracked_stream);
            let send_result = tokio::time::timeout(total_timeout, async {
                client
                    .request(method, &url)
                    .headers(headers)
                    .body(stream_body)
                    .send()
                    .await
            })
            .await;

            let (status, error) = match send_result {
                Ok(Ok(resp)) => {
                    let _ = resp.bytes().await;
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
                    begin.elapsed().as_millis() as u64,
                    bytes_sent.load(Ordering::Relaxed),
                    chunks_sent.load(Ordering::Relaxed),
                    error,
                );
                emit_mirror_log(entry).await;
            }

            record_mirror_metric(status);
        });

        session.set_mirror_state(MirrorState::Streaming {
            body_tx,
            writer_handle,
            channel_full_flag,
            channel_full_timeout_ms,
        });
        log.push("mirror sent");
        PluginRunningResult::GoodNext
    }
}
