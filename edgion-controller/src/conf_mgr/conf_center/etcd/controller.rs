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

use super::leadership_gate::LeadershipGate;
use super::resource_controller::EtcdResourceController;
use super::status::EtcdStatusHandler;
use super::watcher::{EtcdWatcher, KindEventReceiver};
use crate::conf_mgr::conf_center::non_kube_common::{spawn_all_controllers, LocalControllerBackend, LocalTaskScope};
use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
use crate::conf_mgr::sync_runtime::ShutdownSignal;
use crate::conf_mgr::ControllerRuntime;
use edgion_common::link_sys_core::providers::etcd::EtcdLinkClient;
use edgion_resources::ResourceMeta;

/// Backend implementation that wires `EtcdWatcher` and `EtcdResourceController`
/// into the shared [`LocalControllerBackend`] interface.
struct EtcdControllerBackend {
    scope: LocalTaskScope,
    watcher: Arc<EtcdWatcher>,
    status_handler: Arc<EtcdStatusHandler>,
    conf_prefix: Arc<str>,
}

#[async_trait]
impl LocalControllerBackend for EtcdControllerBackend {
    type EventReceiver = KindEventReceiver;

    async fn subscribe(&self, kind: &'static str) -> KindEventReceiver {
        self.watcher.subscribe(kind).await
    }

    async fn prime(&self) -> Result<()> {
        self.watcher.prime().await
    }

    async fn dispatch_init(&self, kinds: &[&'static str]) -> Result<()> {
        self.watcher.dispatch_init(kinds).await
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
        let ctrl = EtcdResourceController::new(
            kind,
            processor,
            self.status_handler.clone(),
            event_rx,
            self.conf_prefix.clone(),
        )
        .with_shutdown(shutdown_signal)
        .with_scope(self.scope.clone());
        self.scope.spawn(async move { ctrl.run().await })
    }
}

/// EtcdController — top-level controller for etcd mode.
pub struct EtcdController {
    controller_name: String,
    client: Arc<EtcdLinkClient>,
    key_prefix: String,
    /// Optional leadership fence threaded into the status handler so a
    /// non-leader controller cannot write resource status to etcd (spec §4.6b).
    gate: Option<LeadershipGate>,
}

impl EtcdController {
    pub fn new(
        client: Arc<EtcdLinkClient>,
        key_prefix: String,
        gate: Option<LeadershipGate>,
        controller_name: String,
    ) -> Self {
        tracing::info!(
            component = "etcd_controller",
            key_prefix = %key_prefix,
            "Creating EtcdController"
        );
        Self {
            controller_name,
            client,
            key_prefix,
            gate,
        }
    }

    /// Run the controller.
    pub async fn run(
        &self,
        shutdown_signal: ShutdownSignal,
        runtime: Arc<ControllerRuntime>,
        scope: LocalTaskScope,
    ) -> Result<()> {
        tracing::info!(
            component = "etcd_controller",
            key_prefix = %self.key_prefix,
            "Starting EtcdController"
        );

        // Create shared components
        let watcher = Arc::new(EtcdWatcher::new(self.client.clone(), &self.key_prefix));
        let status_handler = match &self.gate {
            Some(gate) => Arc::new(EtcdStatusHandler::new_gated(
                self.client.clone(),
                self.key_prefix.clone(),
                gate.clone(),
            )),
            None => Arc::new(EtcdStatusHandler::new(self.client.clone(), self.key_prefix.clone())),
        };

        let backend = EtcdControllerBackend {
            scope,
            watcher: watcher.clone(),
            status_handler,
            conf_prefix: format!("{}/conf/", self.key_prefix).into(),
        };

        // Spawn all resource controllers
        let handles = spawn_all_controllers(
            &backend,
            shutdown_signal.clone(),
            runtime,
            "etcd_controller",
            &self.controller_name,
        )
        .await?;

        tracing::info!(
            component = "etcd_controller",
            count = handles.len(),
            "All ResourceControllers spawned"
        );

        // Run the watcher runtime phase (prime + init already happened in spawn_all_controllers)
        watcher.run_runtime(shutdown_signal).await?;

        // Abort all controllers
        for handle in handles {
            handle.abort();
        }

        tracing::info!(component = "etcd_controller", "EtcdController stopped");
        Ok(())
    }
}
