//! FileSystemResourceController - Per-resource controller for FileSystem mode
//!
//! This controller receives events from FileSystemWatcher's broadcast channel
//! and processes them using the same logic as Kubernetes mode.
//!
//! The event handling logic mirrors K8s mode's ResourceController:
//! - Init -> processor.on_init()
//! - InitApply(content) -> parse + processor.on_init_apply(obj)
//! - InitDone -> processor.on_init_done()
//! - Apply(content) -> parse + processor.on_apply(&obj)
//! - Delete(info) -> processor.on_delete(&obj)

use super::event::FileSystemEvent;
use super::file_watcher::{build_path_from_key, KindEventReceiver};
use crate::core::conf_mgr::sync_runtime::metrics::{controller_metrics, InitSyncTimer};
use crate::core::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
use crate::core::conf_mgr::sync_runtime::ShutdownSignal;
use crate::types::ResourceMeta;
use anyhow::Result;
use kube::Resource;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

/// FileSystemResourceController - handles events for a single resource type
pub struct FileSystemResourceController<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    kind: &'static str,
    processor: Arc<ResourceProcessor<K>>,
    conf_dir: PathBuf,
    event_rx: KindEventReceiver,
    shutdown_signal: Option<ShutdownSignal>,
}

impl<K> FileSystemResourceController<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    /// Create a new FileSystemResourceController
    pub fn new(
        kind: &'static str,
        processor: Arc<ResourceProcessor<K>>,
        conf_dir: PathBuf,
        event_rx: KindEventReceiver,
    ) -> Self {
        Self {
            kind,
            processor,
            conf_dir,
            event_rx,
            shutdown_signal: None,
        }
    }

    /// Set shutdown signal
    pub fn with_shutdown(mut self, signal: ShutdownSignal) -> Self {
        self.shutdown_signal = Some(signal);
        self
    }

    /// Run the controller event loop
    pub async fn run(mut self) -> Result<()> {
        let kind = self.kind;

        controller_metrics().controller_started();
        tracing::info!(
            component = "fs_resource_controller",
            kind = kind,
            "Starting FileSystemResourceController"
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
                            component = "fs_resource_controller",
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
                Ok(event) => {
                    match event {
                        FileSystemEvent::Init => {
                            if init_done {
                                // Already initialized, this might be a re-init
                                tracing::warn!(
                                    component = "fs_resource_controller",
                                    kind = kind,
                                    "Received Init event after init done, ignoring"
                                );
                            } else {
                                tracing::debug!(
                                    component = "fs_resource_controller",
                                    kind = kind,
                                    "Init phase started"
                                );
                                init_timer = Some(InitSyncTimer::start(kind));
                                self.processor.on_init();
                            }
                        }
                        FileSystemEvent::InitApply(content) => {
                            // Parse YAML content to resource type
                            match serde_yaml::from_str::<K>(&content) {
                                Ok(obj) => {
                                    if self.processor.on_init_apply(obj) {
                                        init_count += 1;
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        component = "fs_resource_controller",
                                        kind = kind,
                                        error = %e,
                                        "Failed to parse resource during init"
                                    );
                                }
                            }
                        }
                        FileSystemEvent::InitDone => {
                            let init_duration = init_timer.take().map(|t| t.complete(init_count)).unwrap_or(0.0);
                            tracing::info!(
                                component = "fs_resource_controller",
                                kind = kind,
                                count = init_count,
                                duration_secs = init_duration,
                                "Init phase complete"
                            );

                            // Mark cache ready
                            self.processor.on_init_done();
                            init_done = true;

                            // Spawn worker for runtime phase
                            worker_handle = Some(spawn_worker(
                                self.processor.clone(),
                                self.conf_dir.clone(),
                                kind,
                                self.shutdown_signal.clone(),
                            ));

                            tracing::info!(
                                component = "fs_resource_controller",
                                kind = kind,
                                "Worker started, processing runtime events"
                            );
                        }
                        FileSystemEvent::Apply(content) => {
                            if !init_done {
                                // During init phase, treat as InitApply
                                match serde_yaml::from_str::<K>(&content) {
                                    Ok(obj) => {
                                        if self.processor.on_init_apply(obj) {
                                            init_count += 1;
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            component = "fs_resource_controller",
                                            kind = kind,
                                            error = %e,
                                            "Failed to parse resource during apply"
                                        );
                                    }
                                }
                            } else {
                                // Runtime phase - parse and enqueue
                                match serde_yaml::from_str::<K>(&content) {
                                    Ok(obj) => {
                                        self.processor.on_apply(&obj);
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            component = "fs_resource_controller",
                                            kind = kind,
                                            error = %e,
                                            "Failed to parse resource for apply"
                                        );
                                    }
                                }
                            }
                        }
                        FileSystemEvent::Delete(info) => {
                            if !init_done {
                                tracing::warn!(
                                    component = "fs_resource_controller",
                                    kind = kind,
                                    "Received Delete event during init phase, ignoring"
                                );
                            } else {
                                // Parse delete info: "__DELETE__:kind:key"
                                if let Some(key) = parse_delete_info(&info) {
                                    // For delete, we need to get the cached object
                                    // The worker will handle the actual deletion
                                    if let Some(obj) = self.processor.get(&key) {
                                        self.processor.on_delete(&obj);
                                    } else {
                                        tracing::trace!(
                                            component = "fs_resource_controller",
                                            kind = kind,
                                            key = %key,
                                            "Delete event for non-cached resource, ignoring"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(
                        component = "fs_resource_controller",
                        kind = kind,
                        lagged = n,
                        "Event receiver lagged, some events may be missed"
                    );
                }
                Err(broadcast::error::RecvError::Closed) => {
                    tracing::warn!(
                        component = "fs_resource_controller",
                        kind = kind,
                        "Event channel closed"
                    );
                    break;
                }
            }
        }

        // Wait for worker task to finish
        if let Some(handle) = worker_handle {
            tracing::info!(
                component = "fs_resource_controller",
                kind = kind,
                "Waiting for worker task to finish..."
            );

            match tokio::time::timeout(Duration::from_secs(5), handle).await {
                Ok(Ok(())) => {
                    tracing::info!(
                        component = "fs_resource_controller",
                        kind = kind,
                        "Worker task finished gracefully"
                    );
                }
                Ok(Err(e)) => {
                    tracing::error!(
                        component = "fs_resource_controller",
                        kind = kind,
                        error = %e,
                        "Worker task panicked"
                    );
                }
                Err(_) => {
                    tracing::warn!(
                        component = "fs_resource_controller",
                        kind = kind,
                        "Worker task did not finish within 5 seconds"
                    );
                }
            }
        }

        controller_metrics().controller_stopped();
        tracing::info!(
            component = "fs_resource_controller",
            kind = kind,
            "FileSystemResourceController stopped"
        );

        Ok(())
    }
}

use tokio::sync::broadcast;

/// Spawn worker task for processing workqueue items
fn spawn_worker<K>(
    processor: Arc<ResourceProcessor<K>>,
    conf_dir: PathBuf,
    kind: &'static str,
    shutdown_signal: Option<ShutdownSignal>,
) -> JoinHandle<()>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    let workqueue = processor.workqueue();

    tokio::spawn(async move {
        // Clone shutdown_signal once outside the loop
        let mut shutdown = shutdown_signal;

        loop {
            let item = match &mut shutdown {
                Some(signal) => {
                    tokio::select! {
                        item = workqueue.dequeue() => item,
                        _ = signal.wait() => {
                            tracing::info!(
                                component = "fs_resource_controller",
                                kind = kind,
                                "Worker received shutdown signal"
                            );
                            break;
                        }
                    }
                }
                None => workqueue.dequeue().await,
            };

            match item {
                Some(work_item) => {
                    // For FileSystem mode, we read from file instead of K8s store
                    let path = build_path_from_key(&conf_dir, kind, &work_item.key);

                    let store_obj = if path.exists() {
                        match std::fs::read_to_string(&path) {
                            Ok(content) => match serde_yaml::from_str::<K>(&content) {
                                Ok(obj) => Some(obj),
                                Err(e) => {
                                    tracing::warn!(
                                        component = "fs_resource_controller",
                                        kind = kind,
                                        key = %work_item.key,
                                        error = %e,
                                        "Failed to parse file"
                                    );
                                    None
                                }
                            },
                            Err(e) => {
                                tracing::warn!(
                                    component = "fs_resource_controller",
                                    kind = kind,
                                    key = %work_item.key,
                                    error = %e,
                                    "Failed to read file"
                                );
                                None
                            }
                        }
                    } else {
                        None
                    };

                    // Use processor's process_work_item which handles the reconciliation logic
                    processor.process_work_item(&work_item.key, store_obj);
                    workqueue.done(&work_item.key);
                }
                None => {
                    tracing::warn!(
                        component = "fs_resource_controller",
                        kind = kind,
                        "Workqueue closed, stopping worker"
                    );
                    break;
                }
            }
        }

        tracing::info!(component = "fs_resource_controller", kind = kind, "Worker task ended");
    })
}

/// Parse delete info string: "__DELETE__:kind:key"
fn parse_delete_info(info: &str) -> Option<String> {
    if info.starts_with("__DELETE__:") {
        let parts: Vec<&str> = info.splitn(3, ':').collect();
        if parts.len() == 3 {
            return Some(parts[2].to_string());
        }
    }
    None
}
