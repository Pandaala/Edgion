//! EtcdController — backend-specific adapter for etcd mode.
//!
//! Wires `EtcdWatcher` and `EtcdResourceController` into the shared
//! `LocalControllerBackend` interface so that `spawn_all_controllers` can
//! drive both etcd and FileSystem modes from a single resource list.

use std::fmt::Debug;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use kube::Resource;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::task::JoinHandle;

use super::resource_controller::EtcdResourceController;
use super::status::EtcdStatusHandler;
use super::watcher::{EtcdWatcher, KindEventReceiver};
use crate::core::controller::conf_mgr::conf_center::non_kube_common::{spawn_all_controllers, LocalControllerBackend};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
use crate::core::controller::conf_mgr::sync_runtime::ShutdownSignal;
use crate::core::controller::conf_mgr::ControllerRuntime;
use crate::core::gateway::link_sys::providers::etcd::EtcdLinkClient;
use edgion_resources::ResourceMeta;

/// Backend implementation that wires `EtcdWatcher` and `EtcdResourceController`
/// into the shared [`LocalControllerBackend`] interface.
struct EtcdControllerBackend {
    watcher: Arc<EtcdWatcher>,
    status_handler: Arc<EtcdStatusHandler>,
}

#[async_trait]
impl LocalControllerBackend for EtcdControllerBackend {
    type EventReceiver = KindEventReceiver;

    async fn subscribe(&self, kind: &'static str) -> KindEventReceiver {
        self.watcher.subscribe(kind).await
    }

    fn launch<K>(
        &self,
        kind: &'static str,
        processor: Arc<ResourceProcessor<K>>,
        event_rx: KindEventReceiver,
        shutdown_signal: ShutdownSignal,
    ) -> JoinHandle<Result<()>>
    where
        K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
    {
        let ctrl = EtcdResourceController::new(kind, processor, self.status_handler.clone(), event_rx)
            .with_shutdown(shutdown_signal);
        tokio::spawn(async move { ctrl.run().await })
    }
}

/// EtcdController — top-level controller for etcd mode.
pub struct EtcdController {
    client: Arc<EtcdLinkClient>,
    key_prefix: String,
}

impl EtcdController {
    pub fn new(client: Arc<EtcdLinkClient>, key_prefix: String) -> Self {
        tracing::info!(
            component = "etcd_controller",
            key_prefix = %key_prefix,
            "Creating EtcdController"
        );
        Self { client, key_prefix }
    }

    /// Run the controller.
    pub async fn run(&self, shutdown_signal: ShutdownSignal, runtime: Arc<ControllerRuntime>) -> Result<()> {
        tracing::info!(
            component = "etcd_controller",
            key_prefix = %self.key_prefix,
            "Starting EtcdController"
        );

        // Create shared components
        let watcher = Arc::new(EtcdWatcher::new(self.client.clone(), &self.key_prefix));
        let status_handler = Arc::new(EtcdStatusHandler::new(self.client.clone(), self.key_prefix.clone()));

        let backend = EtcdControllerBackend {
            watcher: watcher.clone(),
            status_handler,
        };

        // Spawn all resource controllers
        let handles = spawn_all_controllers(&backend, shutdown_signal.clone(), runtime, "etcd_controller").await?;

        tracing::info!(
            component = "etcd_controller",
            count = handles.len(),
            "All ResourceControllers spawned"
        );

        // Run the watcher (init phase + runtime phase)
        watcher.run(shutdown_signal).await?;

        // Abort all controllers
        for handle in handles {
            handle.abort();
        }

        tracing::info!(component = "etcd_controller", "EtcdController stopped");
        Ok(())
    }
}
