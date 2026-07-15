//! EtcdCenter — CenterLifeCycle implementation with reload loop.
//!
//! Delegates shared lifecycle logic to `CenterBase` (see `non_kube_common`).
//! The only etcd-specific work is creating `EtcdController` for each iteration.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use arc_swap::ArcSwap;
use tokio_util::sync::CancellationToken;

use super::config::EtcdConfig;
use super::controller::EtcdController;
use super::leader_election::EtcdLeaderElection;
use super::storage::EtcdStorage;
use crate::conf_mgr::conf_center::non_kube_common::CenterBase;
use crate::conf_mgr::conf_center::traits::{
    impl_center_api_delegate, CenterApi, CenterLifeCycle, ConfWriterError, ListOptions, ListResult, ResourceKey,
};
use crate::conf_mgr::sync_runtime::ShutdownHandle;
use crate::conf_mgr::ControllerRuntime;
use crate::conf_sync::conf_server::ConfigSyncServer;
use edgion_common::link_sys_core::providers::etcd::EtcdLinkClient;

/// EtcdCenter — configuration center backed by etcd.
pub struct EtcdCenter {
    config: EtcdConfig,
    writer: EtcdStorage,
    client: Arc<EtcdLinkClient>,
    /// Leader election supervisor. Wrapped in `Arc` so the spawned `run` loop and
    /// the `handle`/`gate` accessors can share one instance (spec §4).
    election: Arc<EtcdLeaderElection>,
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

        // Build the election supervisor. `new` validates the timing/address
        // config and constructs the shared `LeaderState`. The writer is fenced by
        // the election gate so a non-leader controller cannot mutate etcd config.
        let election = Arc::new(EtcdLeaderElection::new(
            client.clone(),
            &config.key_prefix,
            config.leader_election.clone(),
        )?);

        let writer = EtcdStorage::new_gated(client.clone(), config.key_prefix.clone(), election.gate());

        Ok(Self {
            config,
            writer,
            client,
            election,
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

        // Inject conf_writer into runtime so handlers can auto-create resources (e.g. EdgionConfigData).
        // Fenced by the election gate: a non-leader must not mutate etcd config.
        let writer_arc: Arc<dyn CenterApi + Send + Sync> = Arc::new(EtcdStorage::new_gated(
            self.client.clone(),
            self.config.key_prefix.clone(),
            self.election.gate(),
        ));
        self.base.runtime().set_conf_writer(writer_arc);

        // Per-leadership-term stream shutdown token slot. The election installs a
        // fresh token at each campaign win; the gRPC server will read it in Task 10.
        // Nothing consumes it yet, so it is created and handed to `run` as-is.
        let term_slot = Arc::new(ArcSwap::from_pointee(CancellationToken::new()));

        // Spawn the election supervisor. It shares `LeaderState` with the gate
        // injected above and with the lifecycle handle passed below.
        let election = self.election.clone();
        let election_shutdown = shutdown_handle.signal();
        let election_slot = term_slot.clone();
        tokio::spawn(async move {
            election.run(election_shutdown, election_slot).await;
        });

        let client = self.client.clone();
        let key_prefix = self.config.key_prefix.clone();
        let gate = self.election.gate();

        self.base
            .run_lifecycle_gated(
                shutdown_handle,
                "etcd_center",
                Some(self.election.handle()),
                move |shutdown, runtime, error_tx| {
                    let ctrl = EtcdController::new(client.clone(), key_prefix.clone(), Some(gate.clone()));
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
                },
            )
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
