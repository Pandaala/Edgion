//! One OTLP exporter generation owned by the shared persistent runner.

use std::future::{poll_fn, Future};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwapOption;
use edgion_common::dns::ResolverSpec;
use edgion_resources::output::LinkSysRef;
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};
use opentelemetry_sdk::error::{OTelSdkError, OTelSdkResult};
use opentelemetry_sdk::logs::{BatchConfigBuilder, BatchLogProcessor, LogBatch, LogExporter, SdkLoggerProvider};
use parking_lot::Mutex;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::log_sender::emit_record;
use super::transport::PreparedTransport;
use crate::link_sys::runtime::apply::{ConnectionCoverage, RuntimeFingerprint};
use crate::link_sys::runtime::health_probe::{apply_health_result, spawn_health_probe, ProbeConfig};
use crate::link_sys::LogMeta;

type CleanupResult = Result<(), &'static str>;

/// The SDK's standard batch thread does not enter a Tokio runtime. Enter the
/// captured runtime for each poll, and bound the entire RPC including trailers.
struct BoundedExporter {
    inner: Option<opentelemetry_otlp::LogExporter>,
    runtime: tokio::runtime::Handle,
    timeout: Duration,
    cancellation: CancellationToken,
    dropped: Option<oneshot::Sender<()>>,
}

impl std::fmt::Debug for BoundedExporter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OtlpBoundedExporter")
    }
}

impl LogExporter for BoundedExporter {
    async fn export(&self, batch: LogBatch<'_>) -> OTelSdkResult {
        let Some(inner) = &self.inner else {
            return Err(OTelSdkError::AlreadyShutdown);
        };
        let mut operation = std::pin::pin!(async {
            tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => Err(OTelSdkError::AlreadyShutdown),
                _ = tokio::time::sleep(self.timeout) => {
                    Err(OTelSdkError::InternalFailure("export_timeout".into()))
                }
                result = inner.export(batch) => {
                    result.map_err(|_| OTelSdkError::InternalFailure("export_failed".into()))
                }
            }
        });
        poll_fn(|context| {
            let _entered = self.runtime.enter();
            operation.as_mut().poll(context)
        })
        .await
    }

    fn set_resource(&mut self, resource: &opentelemetry_sdk::Resource) {
        if let Some(inner) = &mut self.inner {
            inner.set_resource(resource);
        }
    }
}

impl Drop for BoundedExporter {
    fn drop(&mut self) {
        drop(self.inner.take());
        if let Some(dropped) = self.dropped.take() {
            let _ = dropped.send(());
        }
    }
}

struct Pipeline {
    provider: SdkLoggerProvider,
    cancellation: CancellationToken,
    dropped: Mutex<Option<oneshot::Receiver<()>>>,
}

impl Pipeline {
    fn build(transport: Arc<PreparedTransport>, exporter: opentelemetry_otlp::LogExporter) -> Arc<Self> {
        let cancellation = CancellationToken::new();
        let (dropped_tx, dropped) = oneshot::channel();
        let exporter = BoundedExporter {
            inner: Some(exporter),
            runtime: tokio::runtime::Handle::current(),
            timeout: transport.timeout,
            cancellation: cancellation.clone(),
            dropped: Some(dropped_tx),
        };
        let batch = BatchConfigBuilder::default()
            .with_max_queue_size(2048)
            .with_max_export_batch_size(512)
            .with_scheduled_delay(Duration::from_secs(1))
            .build();
        let processor = BatchLogProcessor::builder(exporter).with_batch_config(batch).build();
        let provider = SdkLoggerProvider::builder().with_log_processor(processor).build();
        Arc::new(Self {
            provider,
            cancellation,
            dropped: Mutex::new(Some(dropped)),
        })
    }
}

/// Start cleanup synchronously and retain the task before any caller awaits.
fn retire(pipeline: Arc<Pipeline>) -> JoinHandle<CleanupResult> {
    let cancellation = pipeline.cancellation.clone();
    let dropped = pipeline.dropped.lock().take();
    tokio::spawn(async move {
        let mut worker = tokio::task::spawn_blocking(move || {
            let result = pipeline.provider.shutdown_with_timeout(Duration::from_secs(30));
            drop(pipeline);
            result.map_err(|_| "shutdown_failed")
        });
        let result = tokio::select! {
            result = &mut worker => result,
            _ = tokio::time::sleep(Duration::from_secs(5)) => {
                cancellation.cancel();
                worker.await
            }
        };
        cancellation.cancel();
        let released = match dropped {
            Some(dropped) => dropped.await.map_err(|_| "shutdown_failed"),
            None => Err("shutdown_failed"),
        };
        result.map_err(|_| "shutdown_failed")?.and(released)
    })
}

/// One prepared and eventually published OTLP generation.
pub struct OtlpLinkClient {
    key: String,
    transport: Arc<PreparedTransport>,
    pipeline: ArcSwapOption<Pipeline>,
    fingerprint: RuntimeFingerprint,
    healthy: Arc<AtomicBool>,
    last_flip_log_ms: AtomicU64,
    retired: Arc<AtomicBool>,
    shutdown_started: AtomicBool,
    probe_shutdown: Mutex<Option<watch::Sender<bool>>>,
    probe: Mutex<Option<JoinHandle<()>>>,
}

impl OtlpLinkClient {
    pub fn from_linksys(key: &str, resource: &LinkSys) -> anyhow::Result<Arc<Self>> {
        Self::from_linksys_inner(key, resource, ResolverSpec::System)
    }

    #[cfg(feature = "test-util")]
    pub fn from_linksys_with_resolver(
        key: &str,
        resource: &LinkSys,
        resolver: ResolverSpec,
    ) -> anyhow::Result<Arc<Self>> {
        Self::from_linksys_inner(key, resource, resolver)
    }

    fn from_linksys_inner(key: &str, resource: &LinkSys, resolver: ResolverSpec) -> anyhow::Result<Arc<Self>> {
        let (namespace, name) = key.split_once('/').ok_or_else(|| anyhow::anyhow!("invalid_config"))?;
        LinkSysRef {
            namespace: namespace.to_owned(),
            name: name.to_owned(),
        }
        .validate()
        .map_err(|_| anyhow::anyhow!("invalid_config"))?;
        if resource.key_name_str() != key || !resource.validate_config().is_empty() {
            anyhow::bail!("invalid_config");
        }
        let SystemConfig::Otlp(config) = &resource.spec.config else {
            anyhow::bail!("OTLP from_linksys called with a non-OTLP LinkSys");
        };
        let transport = PreparedTransport::prepare(config, resolver).map_err(anyhow::Error::msg)?;
        let fingerprint = super::apply::fingerprint_otlp(resource)?;
        Ok(Arc::new(Self {
            key: key.to_string(),
            transport,
            pipeline: ArcSwapOption::empty(),
            fingerprint,
            healthy: Arc::new(AtomicBool::new(false)),
            last_flip_log_ms: AtomicU64::new(0),
            retired: Arc::new(AtomicBool::new(false)),
            shutdown_started: AtomicBool::new(false),
            probe_shutdown: Mutex::new(None),
            probe: Mutex::new(None),
        }))
    }

    pub(crate) fn runtime_fingerprint(&self) -> &RuntimeFingerprint {
        &self.fingerprint
    }

    pub(crate) fn connection_coverage(&self) -> ConnectionCoverage {
        ConnectionCoverage {
            configured: 1,
            live: usize::from(
                self.pipeline.load().is_some() && !self.retired.load(Ordering::Relaxed) && self.healthy(),
            ),
        }
    }

    pub(crate) fn mark_retired(&self) {
        self.retired.store(true, Ordering::Relaxed);
        if let Some(shutdown) = self.probe_shutdown.lock().take() {
            let _ = shutdown.send(true);
        }
        if let Some(probe) = self.probe.lock().take() {
            probe.abort();
        }
    }

    pub async fn init(&self) -> anyhow::Result<()> {
        if self.shutdown_started.load(Ordering::Acquire) || self.retired.load(Ordering::Relaxed) {
            anyhow::bail!("stopped");
        }
        let exporter = self.transport.prepare_exporter().await.map_err(anyhow::Error::msg)?;
        if self.shutdown_started.load(Ordering::Acquire) || self.retired.load(Ordering::Relaxed) {
            anyhow::bail!("stopped");
        }
        self.pipeline
            .store(Some(Pipeline::build(self.transport.clone(), exporter)));
        apply_health_result(&self.healthy, &self.key, "otlp", &self.last_flip_log_ms, true, false);

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let transport = self.transport.clone();
        let retired = self.retired.clone();
        let probe = spawn_health_probe(
            ProbeConfig {
                name: self.key.clone(),
                system_type: "otlp",
                base_interval: Duration::from_secs(10),
                max_backoff: Duration::from_secs(30),
                probe_timeout: Duration::from_secs(8),
                upgrade_only: false,
            },
            self.healthy.clone(),
            shutdown_rx,
            move || {
                let transport = transport.clone();
                let retired = retired.clone();
                async move {
                    if retired.load(Ordering::Relaxed) {
                        return false;
                    }
                    transport.probe().await
                }
            },
        );
        *self.probe_shutdown.lock() = Some(shutdown_tx);
        *self.probe.lock() = Some(probe);
        Ok(())
    }

    pub fn healthy(&self) -> bool {
        !self.retired.load(Ordering::Relaxed)
            && !self.shutdown_started.load(Ordering::Acquire)
            && self.healthy.load(Ordering::Relaxed)
    }

    pub fn name(&self) -> &str {
        &self.key
    }

    pub fn send_with_meta(&self, data: String, meta: &LogMeta<'_>) -> anyhow::Result<()> {
        if !self.healthy() {
            anyhow::bail!("otlp_unavailable");
        }
        let pipeline = self.pipeline.load();
        let pipeline = pipeline.as_ref().ok_or_else(|| anyhow::anyhow!("otlp_unavailable"))?;
        emit_record(&pipeline.provider, data, meta);
        Ok(())
    }

    pub async fn shutdown(&self) -> CleanupResult {
        if self.shutdown_started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.mark_retired();
        self.healthy.store(false, Ordering::Relaxed);
        let Some(pipeline) = self.pipeline.swap(None) else {
            return Ok(());
        };
        retire(pipeline).await.map_err(|_| "shutdown_failed")?
    }

    /// Build a real SDK pipeline without dialing for runtime-store tests.
    #[cfg(test)]
    pub(crate) fn publish_lazy_test_pipeline(&self) -> CleanupResult {
        let channel = tonic014::transport::Endpoint::from_static("http://example.test:4317").connect_lazy();
        let exporter = self.transport.exporter(channel)?;
        self.pipeline
            .store(Some(Pipeline::build(self.transport.clone(), exporter)));
        self.healthy.store(true, Ordering::Relaxed);
        Ok(())
    }
}

impl Drop for OtlpLinkClient {
    fn drop(&mut self) {
        if let Some(shutdown) = self.probe_shutdown.lock().take() {
            let _ = shutdown.send(true);
        }
        if let Some(probe) = self.probe.lock().take() {
            probe.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(name: &str, endpoint: &str) -> LinkSys {
        serde_json::from_value(serde_json::json!({
            "apiVersion": "edgion.io/v1",
            "kind": "LinkSys",
            "metadata": {"namespace": "test", "name": name},
            "spec": {"type": "otlp", "config": {"endpoint": endpoint}}
        }))
        .expect("OTLP fixture")
    }

    #[test]
    fn invalid_keys_never_build_a_generation() {
        let resource = resource("otlp", "http://example.test:4317");
        for key in ["", "raw", "/name", "namespace/", "namespace/a/b", "Bad/name"] {
            assert!(OtlpLinkClient::from_linksys_inner(key, &resource, ResolverSpec::System,).is_err());
        }
    }

    #[tokio::test]
    async fn lazy_pipeline_has_single_live_coverage_and_shutdown_clears_it() {
        let resource = resource("otlp", "http://example.test:4317");
        let client = OtlpLinkClient::from_linksys_inner("test/otlp", &resource, ResolverSpec::System).unwrap();
        assert_eq!(
            client.connection_coverage(),
            ConnectionCoverage { configured: 1, live: 0 }
        );
        client.publish_lazy_test_pipeline().unwrap();
        assert_eq!(
            client.connection_coverage(),
            ConnectionCoverage { configured: 1, live: 1 }
        );
        client.shutdown().await.unwrap();
        assert_eq!(client.connection_coverage().live, 0);
        assert!(client.pipeline.load().is_none());
    }

    #[tokio::test]
    async fn retired_generation_stops_serving_and_health_writes() {
        let resource = resource("retired", "http://example.test:4317");
        let client = OtlpLinkClient::from_linksys_inner("test/retired", &resource, ResolverSpec::System).unwrap();
        client.publish_lazy_test_pipeline().unwrap();
        client.mark_retired();
        assert!(!client.healthy());
        assert!(client
            .send_with_meta("record".to_string(), &LogMeta::default())
            .is_err());
        client.shutdown().await.unwrap();
    }
}
