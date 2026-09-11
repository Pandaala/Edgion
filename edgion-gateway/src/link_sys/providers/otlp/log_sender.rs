use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use async_trait::async_trait;
use edgion_resources::sensitive::apply_sensitive;
use opentelemetry::logs::{AnyValue, LogRecord, Logger, LoggerProvider, Severity};
use opentelemetry_otlp::{LogExporter, WithExportConfig};
use opentelemetry_sdk::logs::SdkLoggerProvider;

use crate::link_sys::{DataSender, LogMeta};
use edgion_resources::output::{OtlpOutputConfig, OtlpProtocol};

fn attach_request_trace_id(record: &mut impl LogRecord, trace_id: Option<[u8; 16]>) {
    if let Some(trace_id) = trace_id {
        // The SDK requires a SpanId; INVALID means no span association.
        // Never label the remote parent as an exported gateway span.
        record.set_trace_context(
            opentelemetry::TraceId::from_bytes(trace_id),
            opentelemetry::SpanId::INVALID,
            None,
        );
    }
}

/// Parse an OTLP endpoint URL into (host, port). v1 is gRPC-only (`init` rejects
/// every non-Grpc protocol), so there is a single supported port; default 4317
/// when absent. Uses `http::Uri` to handle scheme, IPv6 brackets, and paths.
fn parse_otlp_host_port(endpoint: &str) -> Option<(String, u16)> {
    // Require "://" so that bare "host:port" strings (where http::Uri would treat
    // the host as the scheme name) are rejected before parsing.
    if !endpoint.contains("://") {
        return None;
    }
    let uri: http::Uri = endpoint.parse().ok()?;
    let raw_host = uri.host()?;
    // http 1.x returns IPv6 addresses with surrounding brackets (e.g. "[::1]");
    // strip them so callers receive the bare address for TcpStream::connect.
    let host = raw_host.trim_matches(|c| c == '[' || c == ']').to_string();
    let port = uri.port_u16().unwrap_or(4317);
    Some((host, port))
}

/// Connection-level liveness probe for OTLP.
///
/// CONNECTION-LEVEL ONLY: `emit()` is fire-and-forget, so the SDK exposes no
/// export result. A successful TCP connect proves the port is open, NOT that TLS
/// (for an `https` endpoint) or the OTLP/gRPC export pipeline works. The driver
/// bounds this call with `probe_timeout`.
async fn probe_otlp_endpoint(endpoint: &str) -> bool {
    let Some((host, port)) = parse_otlp_host_port(endpoint) else {
        tracing::warn!(otlp = %endpoint, "OTLP probe: cannot parse endpoint (must include a scheme, e.g. http://host:4317); marking unhealthy");
        return false;
    };
    tokio::net::TcpStream::connect((host.as_str(), port)).await.is_ok()
}

pub struct OtlpLogSender {
    config: OtlpOutputConfig,
    provider: OnceLock<SdkLoggerProvider>,
    healthy: Arc<AtomicBool>,
    /// Probe shutdown sender. Dropping it (on struct Drop) stops the probe via
    /// the driver's `changed() == Err` path. NOTE: this sender is effectively
    /// process-lifetime today — `OtlpLogSender` lives in a global `OnceLock`
    /// AccessLogger that is never dropped/hot-reloaded; this wiring is a latent
    /// safety net for if OTLP ever becomes reloadable.
    probe_shutdown: Option<tokio::sync::watch::Sender<bool>>,
}

impl OtlpLogSender {
    pub fn new(config: OtlpOutputConfig) -> Self {
        Self {
            config,
            provider: OnceLock::new(),
            healthy: Arc::new(AtomicBool::new(false)),
            probe_shutdown: None,
        }
    }

    /// Shared handle to this sender's health flag.
    ///
    /// `OtlpLogSender` is stored as a `Box<dyn DataSender>` in `AccessLogger`,
    /// which erases the concrete type, so an inherent `set_healthy(&self)` would
    /// be unreachable. Capture this `Arc` at registration time instead and hand
    /// it to the background probe.
    ///
    /// A background probe drives this flag via TCP connect to the endpoint.
    /// Connection-level only: a successful connect does NOT prove TLS or OTLP
    /// export works. See `probe_shutdown` for task lifetime.
    pub fn health_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.healthy)
    }
}

/// Map a numeric OTel SeverityNumber (0-24) to the SDK enum.
///
/// Returns None when severity is 0 (unset) — caller skips `set_severity_number`.
///
/// NOTE: This groups Trace/Debug/Info/Warn/Error/Fatal into their base variants
/// (e.g. `n=2` maps to `Severity::Trace`, not `Severity::Trace2`). HTTP access
/// log callers only produce 0/9/13/17 (from `severity_from_status`), so no
/// precision is lost in practice. Future callers that emit finer-grained numbers
/// should extend this to the full 24-variant map.
pub(crate) fn numeric_to_severity(n: u32) -> Option<Severity> {
    match n {
        1..=4 => Some(Severity::Trace),
        5..=8 => Some(Severity::Debug),
        9..=12 => Some(Severity::Info),
        13..=16 => Some(Severity::Warn),
        17..=20 => Some(Severity::Error),
        21..=24 => Some(Severity::Fatal),
        _ => None,
    }
}

#[inline]
fn render_sensitive_path(path: &str) -> String {
    apply_sensitive(path, str::to_owned)
}

#[async_trait]
impl DataSender<String> for OtlpLogSender {
    async fn init(&mut self) -> Result<()> {
        // Only gRPC is supported in v1 due to tonic version constraints.
        // The workspace uses tonic 0.12; opentelemetry-otlp pulls tonic 0.14
        // internally. Constructing tonic 0.14 types (e.g. MetadataMap) from
        // gateway code (compiled against tonic 0.12) is a compile-time type
        // mismatch, so HttpProto and custom headers are deferred.
        if self.config.protocol != OtlpProtocol::Grpc {
            anyhow::bail!(
                "OTLP protocol {:?} is not yet supported; only Grpc is implemented in v1",
                self.config.protocol
            );
        }
        if self.config.headers.is_some() {
            tracing::warn!(
                endpoint = %self.config.endpoint,
                "OTLP custom headers are not supported in v1 (tonic version constraint); headers ignored"
            );
        }

        let mut builder = LogExporter::builder().with_tonic().with_endpoint(&self.config.endpoint);
        if let Some(ms) = self.config.timeout_ms {
            builder = builder.with_timeout(Duration::from_millis(ms));
        }
        let exporter = builder
            .build()
            .map_err(|e| anyhow::anyhow!("OTLP exporter build failed for '{}': {}", self.config.endpoint, e))?;

        // Pass the LogExporter directly — the SDK wraps it in a BatchLogProcessor internally.
        let provider = SdkLoggerProvider::builder().with_batch_exporter(exporter).build();

        self.provider.set(provider).ok();
        self.healthy.store(true, Ordering::Relaxed);
        // Baseline health gauge so the series appears even when healthy from startup.
        crate::observe::metrics::registry::record_link_sys_health(&self.config.endpoint, true);

        // Background connection-level health probe.
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let endpoint = self.config.endpoint.clone();
        // drop() makes the intent explicit: the JoinHandle detaches (Tokio does NOT
        // abort on drop), and the watch-sender in `probe_shutdown` is the shutdown path.
        drop(crate::link_sys::runtime::health_probe::spawn_health_probe(
            crate::link_sys::runtime::health_probe::ProbeConfig {
                name: endpoint.clone(),
                system_type: "otlp",
                base_interval: std::time::Duration::from_secs(10),
                max_backoff: std::time::Duration::from_secs(30),
                probe_timeout: std::time::Duration::from_secs(3),
                upgrade_only: false,
            },
            self.health_handle(),
            shutdown_rx,
            move || {
                let endpoint = endpoint.clone();
                async move { probe_otlp_endpoint(&endpoint).await }
            },
        ));
        self.probe_shutdown = Some(shutdown_tx);

        Ok(())
    }

    fn healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    async fn send(&self, data: String) -> Result<()> {
        // Fallback path: no typed meta available (TCP/UDP/TLS log paths that
        // still call send() rather than send_with_meta()).
        let meta = LogMeta {
            timestamp_ns: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
            ..Default::default()
        };
        self.send_with_meta(data, &meta).await
    }

    async fn send_with_meta(&self, data: String, meta: &LogMeta<'_>) -> Result<()> {
        let provider = self
            .provider
            .get()
            .ok_or_else(|| anyhow::anyhow!("OTLP sender '{}' not initialized", self.config.endpoint))?;

        // provider.logger() is cheap (returns a reference clone).
        let logger = provider.logger("edgion-gateway");

        let ts = SystemTime::UNIX_EPOCH + Duration::from_nanos(meta.timestamp_ns);
        let mut record = logger.create_log_record();
        record.set_body(AnyValue::String(data.into()));
        attach_request_trace_id(&mut record, meta.trace_id);
        record.set_timestamp(ts);
        record.set_observed_timestamp(SystemTime::now());
        if let Some(sev) = numeric_to_severity(meta.severity_number) {
            record.set_severity_number(sev);
        }

        if let Some(status) = meta.status_code {
            record.add_attribute("http.response.status_code", status as i64);
        }
        if let Some(method) = meta.method {
            // Materialize ownership here because non-'static &str is not
            // Into<AnyValue> in opentelemetry 0.31.
            record.add_attribute("http.request.method", method.to_owned());
        }
        if let Some(path) = meta.path {
            record.add_attribute("url.path", render_sensitive_path(path));
        }

        logger.emit(record);
        Ok(())
    }

    fn name(&self) -> &str {
        &self.config.endpoint
    }
}

// SdkLoggerProvider's Drop impl calls shutdown(), which blocks until the
// BatchLogProcessor has flushed pending records. When OtlpLogSender is dropped
// during graceful shutdown (via AccessLogger drop), the batch queue is flushed
// automatically.
//
// CAUTION: shutdown() drives the gRPC flush via `futures_executor::block_on`
// while holding an internal lock. On a Tokio current-thread runtime (e.g.
// #[tokio::test] without multi_thread), calling init() and then dropping the
// sender on the runtime thread can deadlock because block_on cannot schedule the
// Tokio IO future. Tests that call init() must use a multi-threaded runtime or
// call shutdown() from a separate thread.

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::sensitive::{SensitiveMode, SENSITIVE_MODE};
    use std::sync::atomic::Ordering;

    #[test]
    fn request_trace_id_is_typed_without_claiming_a_gateway_span() {
        let provider = SdkLoggerProvider::builder().build();
        let logger = provider.logger("request-id-test");
        let mut record = logger.create_log_record();
        attach_request_trace_id(&mut record, None);
        assert!(record.trace_context().is_none());
        attach_request_trace_id(&mut record, Some([7; 16]));
        let context = record.trace_context().unwrap();
        assert_eq!(context.trace_id.to_bytes(), [7; 16]);
        assert_eq!(context.span_id, opentelemetry::SpanId::INVALID);
        assert_eq!(context.trace_flags, None);
    }

    fn assert_sensitive_path_in_subprocess(test_name: &str, mode: SensitiveMode, expected: &str) {
        const CHILD_ENV: &str = "EDGION_OTLP_SENSITIVE_PATH_CHILD";
        const TEST_MODULE: &str = "link_sys::providers::otlp::log_sender::tests";

        if std::env::var(CHILD_ENV).as_deref() == Ok(test_name) {
            // This process runs only the exact test selected below, so changing
            // the process-global mode cannot race another lib test.
            SENSITIVE_MODE.store(mode as u8, Ordering::SeqCst);
            assert_eq!(render_sensitive_path("/customers/alice"), expected);
            return;
        }

        let exact_test_name = format!("{TEST_MODULE}::{test_name}");
        let output = std::process::Command::new(
            std::env::current_exe().expect("the Rust test harness must have a current executable"),
        )
        .args(["--exact", exact_test_name.as_str(), "--nocapture"])
        .env(CHILD_ENV, test_name)
        .output()
        .expect("the isolated OTLP sensitive-path test process must start");
        assert!(
            output.status.success(),
            "isolated OTLP sensitive-path test failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn make_grpc_config() -> OtlpOutputConfig {
        OtlpOutputConfig {
            endpoint: "http://localhost:4317".to_string(),
            protocol: OtlpProtocol::Grpc,
            headers: None,
            timeout_ms: None,
        }
    }

    #[test]
    fn test_otlp_log_sender_name() {
        let sender = OtlpLogSender::new(make_grpc_config());
        assert_eq!(sender.name(), "http://localhost:4317");
    }

    #[test]
    fn test_otlp_log_sender_healthy_before_init() {
        let sender = OtlpLogSender::new(make_grpc_config());
        assert!(!sender.healthy());
    }

    #[test]
    fn health_handle_reflects_and_drives_flag() {
        let sender = OtlpLogSender::new(make_grpc_config());
        let handle = sender.health_handle();
        // Starts unhealthy before init.
        assert!(!sender.healthy());
        // The probe drives the same flag the sender reads.
        handle.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(sender.healthy());
        handle.store(false, std::sync::atomic::Ordering::Relaxed);
        assert!(!sender.healthy());
    }

    #[test]
    fn test_severity_mapping() {
        assert_eq!(numeric_to_severity(0), None);
        assert_eq!(numeric_to_severity(9), Some(Severity::Info));
        assert_eq!(numeric_to_severity(13), Some(Severity::Warn));
        assert_eq!(numeric_to_severity(17), Some(Severity::Error));
        assert_eq!(numeric_to_severity(24), Some(Severity::Fatal));
        assert_eq!(numeric_to_severity(25), None);
    }

    #[test]
    fn url_path_is_clear_when_sensitive_mode_is_off() {
        assert_sensitive_path_in_subprocess(
            "url_path_is_clear_when_sensitive_mode_is_off",
            SensitiveMode::Off,
            "/customers/alice",
        );
    }

    #[test]
    fn url_path_is_replaced_when_sensitive_mode_is_replace() {
        assert_sensitive_path_in_subprocess(
            "url_path_is_replaced_when_sensitive_mode_is_replace",
            SensitiveMode::Replace,
            "***",
        );
    }

    #[test]
    fn url_path_is_wrapped_when_sensitive_mode_is_wrap() {
        assert_sensitive_path_in_subprocess(
            "url_path_is_wrapped_when_sensitive_mode_is_wrap",
            SensitiveMode::Wrap,
            "<E>/customers/alice</E>",
        );
    }

    #[tokio::test]
    async fn test_init_http_proto_returns_error() {
        let mut sender = OtlpLogSender::new(OtlpOutputConfig {
            endpoint: "http://localhost:4318".to_string(),
            protocol: OtlpProtocol::HttpProto,
            headers: None,
            timeout_ms: None,
        });
        let err = sender.init().await.unwrap_err();
        assert!(err.to_string().contains("not yet supported"), "unexpected error: {err}");
        assert!(!sender.healthy(), "sender must not be marked healthy after failed init");
    }

    #[test]
    fn parse_endpoint_variants() {
        assert_eq!(parse_otlp_host_port("http://h:4317"), Some(("h".to_string(), 4317)));
        assert_eq!(parse_otlp_host_port("http://h"), Some(("h".to_string(), 4317))); // default port
        assert_eq!(parse_otlp_host_port("https://c:4317"), Some(("c".to_string(), 4317)));
        assert_eq!(
            parse_otlp_host_port("http://[::1]:4317"),
            Some(("::1".to_string(), 4317))
        );
        assert_eq!(
            parse_otlp_host_port("http://h:4317/v1/logs"),
            Some(("h".to_string(), 4317))
        );
        // Bare host:port has no scheme; http::Uri parses "localhost" as the path
        // component, leaving host() = None, so we correctly return None.
        assert_eq!(parse_otlp_host_port("localhost:4317"), None);
    }

    #[tokio::test]
    async fn probe_true_when_port_open_false_when_closed() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ok_endpoint = format!("http://{}", addr);
        assert!(probe_otlp_endpoint(&ok_endpoint).await);
        // A guaranteed-closed port: bind an ephemeral listener, capture its port, drop it.
        let closed_port = {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        };
        assert!(!probe_otlp_endpoint(&format!("http://127.0.0.1:{}", closed_port)).await);
        // Unparseable -> false, never panic.
        assert!(!probe_otlp_endpoint("").await);
    }
}
