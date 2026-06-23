//! EtcdResourceController — per-kind event loop processing EtcdEvents.
//!
//! Mirrors `FileSystemResourceController` but:
//! - Receives `EtcdEvent` from bounded mpsc channel (backpressure, no event loss)
//! - Uses `EtcdStatusHandler` (async) for status persistence
//! - Worker reads from `LocalRawStore` (updated by event loop) instead of re-fetching from etcd

use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use kube::Resource;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::task::JoinHandle;

use super::event::EtcdEvent;
use super::status::EtcdStatusHandler;
use super::watcher::KindEventReceiver;
use crate::conf_mgr::conf_center::common::parse_resource_content;
use crate::conf_mgr::sync_runtime::metrics::{controller_metrics, InitSyncTimer};
use crate::conf_mgr::sync_runtime::resource_processor::raw_store::RawStore;
use crate::conf_mgr::sync_runtime::resource_processor::{
    extract_status_value, make_resource_key, LocalRawStore, ResourceProcessor, WorkItemResult,
};
use crate::conf_mgr::sync_runtime::ShutdownSignal;
use edgion_resources::ResourceMeta;

/// EtcdResourceController — handles events for a single resource type.
pub struct EtcdResourceController<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    kind: &'static str,
    processor: Arc<ResourceProcessor<K>>,
    status_handler: Arc<EtcdStatusHandler>,
    event_rx: KindEventReceiver,
    shutdown_signal: Option<ShutdownSignal>,
    fs_raw_store: Arc<LocalRawStore<K>>,
}

impl<K> EtcdResourceController<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    pub fn new(
        kind: &'static str,
        processor: Arc<ResourceProcessor<K>>,
        status_handler: Arc<EtcdStatusHandler>,
        event_rx: KindEventReceiver,
    ) -> Self {
        let fs_raw_store = Arc::new(LocalRawStore::new());
        let raw_store: Arc<dyn RawStore<K>> = fs_raw_store.clone();
        processor.set_raw_store(raw_store.clone());
        crate::observe::status_metrics::register_status_renderer::<K>(kind, raw_store);
        Self {
            kind,
            processor,
            status_handler,
            event_rx,
            shutdown_signal: None,
            fs_raw_store,
        }
    }

    pub fn with_shutdown(mut self, signal: ShutdownSignal) -> Self {
        self.shutdown_signal = Some(signal);
        self
    }

    /// Run the event loop.
    pub async fn run(mut self) -> Result<()> {
        let kind = self.kind;

        controller_metrics().controller_started();
        tracing::info!(
            component = "etcd_resource_controller",
            kind = kind,
            "Starting EtcdResourceController"
        );

        let mut init_done = false;
        let mut init_count: usize = 0;
        let mut init_timer: Option<InitSyncTimer> = None;
        let mut worker_handle: Option<JoinHandle<()>> = None;
        let mut shutdown = self.shutdown_signal.clone();

        loop {
            let event = if let Some(ref mut signal) = shutdown {
                tokio::select! {
                    event = self.event_rx.recv() => event,
                    _ = signal.wait() => {
                        tracing::info!(
                            component = "etcd_resource_controller",
                            kind = kind,
                            "Shutdown signal received"
                        );
                        break;
                    }
                }
            } else {
                self.event_rx.recv().await
            };

            match event {
                Some(event) => match event {
                    EtcdEvent::Init => {
                        if !init_done {
                            tracing::debug!(
                                component = "etcd_resource_controller",
                                kind = kind,
                                "Init phase started"
                            );
                            init_timer = Some(InitSyncTimer::start(kind));
                            self.processor.on_init();
                        }
                    }
                    EtcdEvent::InitApply { key, content } => match parse_resource_content::<K>(&content) {
                        Ok(obj) => {
                            let rkey = make_resource_key(&obj);
                            self.fs_raw_store.upsert(rkey.clone(), Arc::new(obj.clone()));

                            let existing_status_json = self.status_handler.read_status_json(kind, &rkey).await;

                            let result = self.processor.on_init_apply(obj, existing_status_json).await;

                            if let WorkItemResult::Processed { obj, status_changed } = result {
                                init_count += 1;
                                if status_changed {
                                    self.persist_status(kind, &rkey, &obj).await;
                                }
                            }
                        }
                        Err(e) => {
                            let rkey = extract_key_from_etcd_key(&key);
                            if let Some(rkey) = rkey {
                                let _ = self
                                    .status_handler
                                    .write_error_status(kind, &rkey, "ParseError", &e.to_string())
                                    .await;
                            }
                            tracing::warn!(
                                component = "etcd_resource_controller",
                                kind = kind,
                                key = %key,
                                error = %e,
                                "Failed to parse resource during init"
                            );
                        }
                    },
                    EtcdEvent::InitDone => {
                        let init_duration = init_timer.take().map(|t| t.complete(init_count)).unwrap_or(0.0);
                        tracing::info!(
                            component = "etcd_resource_controller",
                            kind = kind,
                            count = init_count,
                            duration_secs = init_duration,
                            "Init phase complete"
                        );

                        self.processor.on_init_done();
                        init_done = true;

                        worker_handle = Some(spawn_worker(
                            self.processor.clone(),
                            kind,
                            self.shutdown_signal.clone(),
                            self.fs_raw_store.clone(),
                            self.status_handler.clone(),
                        ));
                    }
                    EtcdEvent::Apply { key, content } => {
                        if !init_done {
                            // During init, treat as InitApply
                            if let Ok(obj) = parse_resource_content::<K>(&content) {
                                let rkey = make_resource_key(&obj);
                                self.fs_raw_store.upsert(rkey.clone(), Arc::new(obj.clone()));
                                let existing_status_json = self.status_handler.read_status_json(kind, &rkey).await;
                                let result = self.processor.on_init_apply(obj, existing_status_json).await;
                                if let WorkItemResult::Processed { obj, status_changed } = result {
                                    init_count += 1;
                                    if status_changed {
                                        self.persist_status(kind, &rkey, &obj).await;
                                    }
                                }
                            }
                        } else {
                            match parse_resource_content::<K>(&content) {
                                Ok(obj) => {
                                    let rkey = make_resource_key(&obj);
                                    self.fs_raw_store.upsert(rkey, Arc::new(obj.clone()));
                                    self.processor.on_apply(&obj).await;
                                }
                                Err(e) => {
                                    if let Some(rkey) = extract_key_from_etcd_key(&key) {
                                        self.fs_raw_store.remove(&rkey);
                                    }
                                    tracing::warn!(
                                        component = "etcd_resource_controller",
                                        kind = kind,
                                        key = %key,
                                        error = %e,
                                        "Failed to parse resource for apply"
                                    );
                                }
                            }
                        }
                    }
                    EtcdEvent::Delete { key } => {
                        if init_done {
                            if let Some(rkey) = extract_key_from_etcd_key(&key) {
                                self.fs_raw_store.remove(&rkey);
                                if let Some(obj) = self.processor.get(&rkey) {
                                    self.processor.on_delete(&obj).await;
                                }
                            }
                        }
                    }
                },
                None => {
                    tracing::warn!(
                        component = "etcd_resource_controller",
                        kind = kind,
                        "Event channel closed"
                    );
                    break;
                }
            }
        }

        // Wait for worker
        if let Some(handle) = worker_handle {
            match tokio::time::timeout(Duration::from_secs(5), handle).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::error!(
                        component = "etcd_resource_controller",
                        kind = kind,
                        error = %e,
                        "Worker task panicked"
                    );
                }
                Err(_) => {
                    tracing::warn!(
                        component = "etcd_resource_controller",
                        kind = kind,
                        "Worker task did not finish within 5 seconds"
                    );
                }
            }
        }

        controller_metrics().controller_stopped();
        tracing::info!(
            component = "etcd_resource_controller",
            kind = kind,
            "EtcdResourceController stopped"
        );
        Ok(())
    }

    /// Persist status to etcd.
    async fn persist_status(&self, kind: &str, key: &str, obj: &K) {
        if let Some(status_value) = extract_status_value(obj) {
            match self.status_handler.write_status_value(kind, key, &status_value).await {
                Ok(()) => {
                    crate::conf_mgr::sync_runtime::record_status_write(kind, "etcd");
                }
                Err(e) => {
                    crate::conf_mgr::sync_runtime::record_status_write_error(kind, "etcd");
                    tracing::warn!(
                        component = "etcd_resource_controller",
                        kind = kind,
                        key = %key,
                        error = %e,
                        "Failed to write status to etcd"
                    );
                }
            }
        }
    }
}

/// Spawn worker task for processing workqueue items.
///
/// Unlike FileSystem mode which re-reads files, the etcd worker uses
/// the `LocalRawStore` which is always up-to-date from the event loop.
fn spawn_worker<K>(
    processor: Arc<ResourceProcessor<K>>,
    kind: &'static str,
    shutdown_signal: Option<ShutdownSignal>,
    fs_raw_store: Arc<LocalRawStore<K>>,
    status_handler: Arc<EtcdStatusHandler>,
) -> JoinHandle<()>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    let workqueue = processor.workqueue();

    tokio::spawn(async move {
        let mut shutdown = shutdown_signal;

        loop {
            let item = match &mut shutdown {
                Some(signal) => {
                    tokio::select! {
                        item = workqueue.dequeue() => item,
                        _ = signal.wait() => break,
                    }
                }
                None => workqueue.dequeue().await,
            };

            match item {
                Some(work_item) => {
                    // Get the latest object from raw_store (updated by event loop)
                    let store_obj = fs_raw_store.get(&work_item.key).map(|arc_obj| (*arc_obj).clone());

                    let existing_status_json = status_handler.read_status_json(kind, &work_item.key).await;

                    let result = processor
                        .process_work_item(
                            &work_item.key,
                            store_obj,
                            existing_status_json,
                            work_item.trigger_chain.clone(),
                        )
                        .await;

                    match result {
                        WorkItemResult::Processed { obj, status_changed } => {
                            if status_changed {
                                if let Some(status_value) = extract_status_value(&obj) {
                                    match status_handler
                                        .write_status_value(kind, &work_item.key, &status_value)
                                        .await
                                    {
                                        Ok(()) => {
                                            crate::conf_mgr::sync_runtime::record_status_write(kind, "etcd");
                                        }
                                        Err(e) => {
                                            crate::conf_mgr::sync_runtime::record_status_write_error(kind, "etcd");
                                            tracing::warn!(
                                                component = "etcd_resource_controller",
                                                kind = kind,
                                                key = %work_item.key,
                                                error = %e,
                                                "Failed to write status to etcd"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        WorkItemResult::Deleted { key } => {
                            let _ = status_handler.delete_status(kind, &key).await;
                        }
                        WorkItemResult::Skipped => {}
                    }

                    workqueue.done(&work_item.key);
                }
                None => break,
            }
        }

        tracing::info!(component = "etcd_resource_controller", kind = kind, "Worker task ended");
    })
}

/// Extract resource key ("namespace/name" or "name") from etcd key.
///
/// Key format: `{prefix}/conf/{kind}/{namespace}/{name}`
/// e.g. "/edgion/conf/HTTPRoute/default/my-route" → "default/my-route"
///      "/edgion/conf/GatewayClass/_/edgion"       → "edgion"
fn extract_key_from_etcd_key(etcd_key: &str) -> Option<String> {
    // Find the /conf/{kind}/ part and extract namespace/name after it
    let conf_idx = etcd_key.find("/conf/")?;
    let after_conf = &etcd_key[conf_idx + 6..]; // skip "/conf/"
                                                // after_conf = "{kind}/{namespace}/{name}"
    let first_slash = after_conf.find('/')?;
    let ns_and_name = &after_conf[first_slash + 1..]; // "{namespace}/{name}"
    let parts: Vec<&str> = ns_and_name.splitn(2, '/').collect();
    if parts.len() != 2 {
        return None;
    }
    let (ns, name) = (parts[0], parts[1]);
    if ns == "_" {
        Some(name.to_string())
    } else {
        Some(format!("{}/{}", ns, name))
    }
}
