//! One OTLP owner per LinkSys key, with last-known-good publication and joined cleanup.

use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use arc_swap::ArcSwapOption;
use edgion_common::dns::ResolverSpec;
use edgion_resources::output::LinkSysRef;
use edgion_resources::resources::link_sys::otlp::OtlpClientConfig;
use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use opentelemetry_sdk::error::{OTelSdkError, OTelSdkResult};
use opentelemetry_sdk::logs::{BatchConfigBuilder, BatchLogProcessor, LogBatch, LogExporter, SdkLoggerProvider};
use parking_lot::Mutex;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::log_sender::emit_record;
use super::transport::PreparedTransport;
use crate::link_sys::runtime::health_probe::{spawn_health_probe, ProbeConfig};
use crate::link_sys::LogMeta;
use crate::observe::metrics::registry::record_link_sys_health;

type CleanupResult = Result<(), &'static str>;

/// The SDK's standard batch thread does not enter a Tokio runtime. Enter the
/// captured runtime for each poll, and bound the ENTIRE RPC including trailers.
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
                _ = tokio::time::sleep(self.timeout) => Err(OTelSdkError::InternalFailure("export_timeout".into())),
                result = inner.export(batch) => result.map_err(|_| OTelSdkError::InternalFailure("export_failed".into())),
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
        // Tonic exporter shutdown is a no-op. Drop its channel before signalling
        // completion; a timeout/SDK shutdown return alone does not prove release.
        drop(self.inner.take());
        if let Some(dropped) = self.dropped.take() {
            let _ = dropped.send(());
        }
    }
}

struct Pipeline {
    transport: Arc<PreparedTransport>,
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
            transport,
            provider,
            cancellation,
            dropped: Mutex::new(Some(dropped)),
        })
    }
}

/// Start cleanup synchronously and retain the returned task BEFORE any caller
/// awaits. Its drain timer survives cancellation of a probe or removal waiter.
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
        // Also stop any export after an unexpected SDK shutdown error. Dropping
        // its provider closes the control queue; the worker then drops exporter.
        cancellation.cancel();
        let released = match dropped {
            Some(dropped) => dropped.await.map_err(|_| "shutdown_failed"),
            None => Err("shutdown_failed"),
        };
        result.map_err(|_| "shutdown_failed")?.and(released)
    })
}

#[derive(Default)]
struct State {
    revision: u64,
    desired: Option<(u64, Arc<PreparedTransport>)>,
    stopped: bool,
    retiring: Option<JoinHandle<CleanupResult>>,
    cleanup_failed: bool,
}

#[derive(Default)]
struct ShutdownState {
    task: Option<Shared<BoxFuture<'static, CleanupResult>>>,
}

/// A validated key owns exactly one probe and one active SDK pipeline. Readers
/// borrow the active immutable generation without taking a blocking lock.
pub struct OtlpLinkClient {
    key: String,
    weak: Weak<Self>,
    resolver: ResolverSpec,
    active: ArcSwapOption<Pipeline>,
    state: Mutex<State>,
    healthy: Arc<AtomicBool>,
    stopped: AtomicBool,
    shutdown_tx: watch::Sender<bool>,
    probe: Mutex<Option<JoinHandle<()>>>,
    shutdown: Mutex<ShutdownState>,
}

impl OtlpLinkClient {
    pub fn new(key: String) -> Result<Arc<Self>, &'static str> {
        Self::new_inner(key, true, ResolverSpec::System)
    }

    /// Integration-only DNS selection; production construction always uses System.
    #[cfg(feature = "test-util")]
    pub fn new_with_resolver(key: String, resolver: ResolverSpec) -> Result<Arc<Self>, &'static str> {
        Self::new_inner(key, true, resolver)
    }

    fn new_inner(key: String, start_probe: bool, resolver: ResolverSpec) -> Result<Arc<Self>, &'static str> {
        let (namespace, name) = key.split_once('/').ok_or("invalid_config")?;
        LinkSysRef {
            namespace: namespace.to_owned(),
            name: name.to_owned(),
        }
        .validate()
        .map_err(|_| "invalid_config")?;
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let client = Arc::new_cyclic(|weak| Self {
            key,
            weak: weak.clone(),
            resolver,
            active: ArcSwapOption::empty(),
            state: Mutex::new(State::default()),
            healthy: Arc::new(AtomicBool::new(false)),
            stopped: AtomicBool::new(false),
            shutdown_tx,
            probe: Mutex::new(None),
            shutdown: Mutex::new(ShutdownState::default()),
        });
        record_link_sys_health(&client.key, false);
        let weak = client.weak.clone();
        if start_probe {
            *client.probe.lock() = Some(spawn_health_probe(
                ProbeConfig {
                    name: client.key.clone(),
                    system_type: "otlp",
                    base_interval: Duration::from_secs(10),
                    max_backoff: Duration::from_secs(30),
                    probe_timeout: Duration::from_secs(8),
                    upgrade_only: false,
                },
                client.healthy.clone(),
                shutdown_rx,
                move || {
                    let weak = weak.clone();
                    async move {
                        match weak.upgrade() {
                            Some(client) => client.probe_once().await,
                            None => false,
                        }
                    }
                },
            ));
        }
        Ok(client)
    }

    /// Invalidation occurs before preparation, so a failed newer update cannot
    /// leave an older pending candidate that later replaces the active pipeline.
    pub fn update(&self, config: &OtlpClientConfig) -> CleanupResult {
        let revision = {
            let mut state = self.state.lock();
            if state.stopped {
                return Err("stopped");
            }
            state.desired = None;
            state.revision = state.revision.checked_add(1).ok_or("invalid_config")?;
            state.revision
        };
        let prepared = PreparedTransport::prepare(config, self.resolver.clone())?;
        let mut state = self.state.lock();
        if state.stopped {
            return Err("stopped");
        }
        if state.revision == revision {
            state.desired = Some((revision, prepared));
        }
        Ok(())
    }

    pub fn invalidate_pending(&self) {
        let mut state = self.state.lock();
        state.desired = None;
        state.revision = state.revision.saturating_add(1);
    }

    pub fn healthy(&self) -> bool {
        !self.stopped.load(Ordering::Acquire) && self.healthy.load(Ordering::Relaxed)
    }

    pub fn send_with_meta(&self, data: String, meta: &LogMeta<'_>) -> anyhow::Result<()> {
        if self.stopped.load(Ordering::Acquire) {
            anyhow::bail!("otlp_unavailable");
        }
        let active = self.active.load();
        let pipeline = active.as_ref().ok_or_else(|| anyhow::anyhow!("otlp_unavailable"))?;
        emit_record(&pipeline.provider, data, meta);
        Ok(())
    }

    /// Reap only a finished retirement without awaiting inside the cancellable
    /// probe. Publication remains blocked while any retirement is outstanding.
    fn reap_retirement(state: &mut State) {
        let Some(task) = &mut state.retiring else {
            return;
        };
        let mut context = Context::from_waker(Waker::noop());
        if let Poll::Ready(result) = Pin::new(task).poll(&mut context) {
            state.cleanup_failed |= !matches!(result, Ok(Ok(())));
            state.retiring = None;
        }
    }

    async fn probe_once(&self) -> bool {
        let desired = {
            let mut state = self.state.lock();
            if state.stopped {
                return false;
            }
            Self::reap_retirement(&mut state);
            if state.retiring.is_none() {
                state.desired.clone()
            } else {
                None
            }
        };
        if let Some((revision, prepared)) = desired {
            if let Ok(exporter) = prepared.prepare_exporter().await {
                if self.publish(revision, prepared, exporter) {
                    return true;
                }
            }
        }
        // A failed pending connection gets its own 3s deadline. It cannot
        // consume the independent 3s old-generation liveness decision.
        let active = self.active.load_full();
        match active {
            Some(active) => active.transport.probe().await,
            None => false,
        }
    }

    fn publish(
        &self,
        revision: u64,
        prepared: Arc<PreparedTransport>,
        exporter: opentelemetry_otlp::LogExporter,
    ) -> bool {
        let mut state = self.state.lock();
        if state.stopped
            || state.retiring.is_some()
            || state.desired.as_ref().is_none_or(|(current, _)| *current != revision)
        {
            return false;
        }
        let pipeline = Pipeline::build(prepared, exporter);
        let old = self.active.swap(Some(pipeline));
        state.desired = None;
        if let Some(old) = old {
            state.retiring = Some(retire(old));
        }
        true
    }

    /// All callers join the same owned cleanup task. Cancelling a waiter leaves
    /// its JoinHandle in the owner, preserving cleanup and subsequent joins.
    /// Publish through the real SDK pipeline without dialing or enqueueing data.
    /// Unit callers use a current-thread runtime and call this before yielding.
    #[cfg(test)]
    pub(crate) fn publish_lazy_test_pipeline(&self) -> CleanupResult {
        let (revision, prepared) = self.state.lock().desired.clone().ok_or("invalid_config")?;
        let channel = tonic014::transport::Endpoint::from_static("http://example.test:4317").connect_lazy();
        let exporter = prepared.exporter(channel)?;
        if self.publish(revision, prepared, exporter) {
            Ok(())
        } else {
            Err("invalid_config")
        }
    }

    /// Register stopped state and owned cleanup synchronously in source-event
    /// order. The caller may then remove the runtime entry and await shutdown.
    pub fn begin_shutdown(&self) -> CleanupResult {
        let mut shutdown = self.shutdown.lock();
        if shutdown.task.is_none() {
            let owner = self.weak.upgrade().ok_or("stopped")?;
            {
                let mut state = self.state.lock();
                state.stopped = true;
                state.desired = None;
            }
            self.stopped.store(true, Ordering::Release);
            self.shutdown_tx.send_replace(true);
            let task = tokio::spawn(async move { owner.finish_shutdown().await });
            // Shared retains the JoinHandle and wakes EVERY concurrent waiter.
            // Cancelling one waiter cannot detach or steal cleanup.
            shutdown.task = Some(
                task.map(|result| result.unwrap_or(Err("shutdown_failed")))
                    .boxed()
                    .shared(),
            );
        }
        Ok(())
    }

    pub async fn shutdown(&self) -> CleanupResult {
        self.begin_shutdown()?;
        let task = self.shutdown.lock().task.as_ref().cloned().ok_or("shutdown_failed")?;
        task.await
    }

    async fn finish_shutdown(&self) -> CleanupResult {
        poll_fn(|context| {
            let mut probe = self.probe.lock();
            let Some(task) = &mut *probe else {
                return Poll::Ready(());
            };
            match Pin::new(task).poll(context) {
                Poll::Ready(_) => {
                    *probe = None;
                    Poll::Ready(())
                }
                Poll::Pending => Poll::Pending,
            }
        })
        .await;
        // Reset only after the driver has exited; an in-flight successful
        // result cannot restore health after this terminal transition.
        self.healthy.store(false, Ordering::Relaxed);
        record_link_sys_health(&self.key, false);
        self.join_retirement().await;
        {
            let mut state = self.state.lock();
            if let Some(active) = self.active.swap(None) {
                state.retiring = Some(retire(active));
            }
        }
        self.join_retirement().await;
        if self.state.lock().cleanup_failed {
            Err("shutdown_failed")
        } else {
            Ok(())
        }
    }

    async fn join_retirement(&self) {
        poll_fn(|context| {
            let mut state = self.state.lock();
            let Some(task) = &mut state.retiring else {
                return Poll::Ready(());
            };
            match Pin::new(task).poll(context) {
                Poll::Ready(result) => {
                    state.cleanup_failed |= !matches!(result, Ok(Ok(())));
                    state.retiring = None;
                    Poll::Ready(())
                }
                Poll::Pending => Poll::Pending,
            }
        })
        .await;
    }
}

impl Drop for OtlpLinkClient {
    fn drop(&mut self) {
        self.shutdown_tx.send_replace(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(endpoint: &str) -> OtlpClientConfig {
        OtlpClientConfig {
            endpoint: endpoint.into(),
            timeout_ms: 1000,
            auth: None,
            tls: None,
        }
    }
    fn desired(client: &OtlpLinkClient) -> (u64, Arc<PreparedTransport>) {
        client.state.lock().desired.clone().unwrap()
    }
    fn exporter() -> opentelemetry_otlp::LogExporter {
        // The channel is lazy; these tests never enqueue an export or dial.
        let channel = tonic014::transport::Endpoint::from_static("http://example.test:4317").connect_lazy();
        let prepared = PreparedTransport::prepare(&config("http://example.test:4317"), ResolverSpec::System).unwrap();
        prepared.exporter(channel).unwrap()
    }
    fn publish(client: &OtlpLinkClient) {
        let (revision, prepared) = desired(client);
        assert!(client.publish(revision, prepared, exporter()));
    }

    #[tokio::test]
    async fn invalid_keys_never_start_an_owner() {
        for key in ["", "raw", "/name", "namespace/", "namespace/a/b", "Bad/name"] {
            assert!(matches!(
                OtlpLinkClient::new_inner(key.into(), false, ResolverSpec::System),
                Err("invalid_config")
            ));
        }
    }

    #[tokio::test]
    async fn failed_update_clears_pending_and_preserves_active() {
        let client = OtlpLinkClient::new_inner("test/otlp".into(), false, ResolverSpec::System).unwrap();
        client.update(&config("http://example.test:4317")).unwrap();
        publish(&client);
        let active = client.active.load_full().unwrap();
        client.update(&config("http://new.example.test:4317")).unwrap();
        let (old_revision, old_prepared) = desired(&client);
        assert_eq!(
            client.update(&config("http://user:secret@example.test")),
            Err("invalid_config")
        );
        assert!(client.state.lock().desired.is_none());
        assert!(!client.publish(old_revision, old_prepared, exporter()));
        assert!(Arc::ptr_eq(&active, client.active.load_full().as_ref().unwrap()));
        drop(active);
        client.shutdown().await.unwrap();
        assert!(client.active.load().is_none());
        client.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn only_current_generation_publishes_and_retirement_gates_next() {
        let client = OtlpLinkClient::new_inner("test/otlp".into(), false, ResolverSpec::System).unwrap();
        client.update(&config("http://old.example.test:4317")).unwrap();
        let (revision, prepared) = desired(&client);
        client.update(&config("http://new.example.test:4317")).unwrap();
        assert!(!client.publish(revision, prepared, exporter()));
        publish(&client);
        client.update(&config("http://next.example.test:4317")).unwrap();
        publish(&client);
        assert!(client.state.lock().retiring.is_some());
        client.update(&config("http://latest.example.test:4317")).unwrap();
        let (revision, prepared) = desired(&client);
        assert!(!client.publish(revision, prepared.clone(), exporter()));
        client.join_retirement().await;
        assert!(client.publish(revision, prepared, exporter()));
        client.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_invalidates_preparation_and_prevents_updates() {
        let client = OtlpLinkClient::new_inner("test/otlp".into(), false, ResolverSpec::System).unwrap();
        client.update(&config("http://example.test:4317")).unwrap();
        let (revision, prepared) = desired(&client);
        client.shutdown().await.unwrap();
        assert!(!client.publish(revision, prepared, exporter()));
        assert_eq!(client.update(&config("http://example.test:4317")), Err("stopped"));
        assert!(!client.healthy());
    }
    #[tokio::test]
    async fn concurrent_shutdown_waiters_all_complete_after_cancellation() {
        let client = OtlpLinkClient::new_inner("test/otlp".into(), false, ResolverSpec::System).unwrap();
        client.update(&config("http://example.test:4317")).unwrap();
        publish(&client);
        let mut waiters = Vec::new();
        for _ in 0..8 {
            let client = client.clone();
            waiters.push(tokio::spawn(async move { client.shutdown().await }));
        }
        waiters.pop().unwrap().abort();
        for waiter in waiters {
            tokio::time::timeout(Duration::from_secs(2), waiter)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
        client.shutdown().await.unwrap();
        assert!(!client.healthy());
        assert!(client.active.load().is_none());
    }
}
