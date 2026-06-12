//! EtcdCenter — CenterLifeCycle implementation with reload loop.
//!
//! Delegates shared lifecycle logic to `CenterBase` (see `non_kube_common`).
//! The only etcd-specific work is creating `EtcdController` for each iteration.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use super::config::EtcdConfig;
use super::controller::EtcdController;
use super::storage::EtcdStorage;
use crate::conf_mgr::conf_center::non_kube_common::CenterBase;
use crate::conf_mgr::conf_center::traits::{
    impl_center_api_delegate, CenterApi, CenterLifeCycle, ConfWriterError, ListOptions, ListResult, ResourceKey,
};
use crate::conf_mgr::sync_runtime::ShutdownHandle;
use crate::conf_mgr::ControllerRuntime;
use crate::conf_sync::conf_server::ConfigSyncServer;
use edgion_common::link::providers::etcd::EtcdLinkClient;

/// EtcdCenter — configuration center backed by etcd.
pub struct EtcdCenter {
    config: EtcdConfig,
    writer: EtcdStorage,
    client: Arc<EtcdLinkClient>,
    base: CenterBase,
}

impl EtcdCenter {
    /// Create a new EtcdCenter.
    ///
    /// This connects to etcd and initializes the client.
    pub async fn new(config: EtcdConfig) -> Result<Self> {
        tracing::info!(
            component = "etcd_center",
            endpoints = ?config.endpoints,
            key_prefix = %config.key_prefix,
            "Creating EtcdCenter"
        );

        // Convert YAML config into the `EtcdClientConfig` + `CredentialState`
        // pair expected by `EtcdLinkClient::from_config`. The state encodes
        // whether the YAML file supplied `conf_center.auth` — `Resolved` if
        // so, `NotRequired` otherwise.
        let (client_config, auth_state) = config.to_client_config()?;
        let client = EtcdLinkClient::from_config("conf-center", &client_config, &auth_state)?;
        client.init().await?;
        let client = Arc::new(client);

        let writer = EtcdStorage::new(client.clone(), config.key_prefix.clone());

        Ok(Self {
            config,
            writer,
            client,
            base: CenterBase::new(),
        })
    }
}

// CenterApi delegates to writer
impl_center_api_delegate!(EtcdCenter, writer);

#[async_trait]
impl CenterLifeCycle for EtcdCenter {
    async fn start(&self, shutdown_handle: ShutdownHandle) -> Result<()> {
        tracing::info!(component = "etcd_center", mode = "etcd", "Starting Etcd lifecycle");

        // Inject conf_writer into runtime so handlers can auto-create resources (e.g. PluginMetaData).
        let writer_arc: Arc<dyn CenterApi + Send + Sync> =
            Arc::new(EtcdStorage::new(self.client.clone(), self.config.key_prefix.clone()));
        self.base.runtime().set_conf_writer(writer_arc);

        let client = self.client.clone();
        let key_prefix = self.config.key_prefix.clone();

        self.base
            .run_lifecycle(shutdown_handle, "etcd_center", |shutdown, runtime, error_tx| {
                let ctrl = EtcdController::new(client.clone(), key_prefix.clone());
                tokio::spawn(async move {
                    if let Err(e) = ctrl.run(shutdown, runtime).await {
                        tracing::error!(
                            component = "etcd_center",
                            error = %e,
                            "EtcdController error"
                        );
                        let _ = error_tx.send(e.to_string());
                    }
                })
            })
            .await
    }

    fn is_ready(&self) -> bool {
        self.base.is_ready()
    }

    fn config_sync_server(&self) -> Option<Arc<ConfigSyncServer>> {
        self.base.config_sync_server()
    }

    fn is_k8s_mode(&self) -> bool {
        false
    }

    fn request_reload(&self) -> Result<(), String> {
        self.base.request_reload()
    }

    fn runtime(&self) -> Arc<ControllerRuntime> {
        self.base.runtime()
    }
}
