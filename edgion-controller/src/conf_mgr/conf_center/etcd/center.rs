//! EtcdCenter — CenterLifeCycle implementation with leadership-scoped iterations.
//!
//! Delegates shared lifecycle logic to `CenterBase` (see `non_kube_common`).
//! The only etcd-specific work is creating `EtcdController` for each iteration.

use std::sync::Arc;
use std::time::Duration;

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
    impl_center_api_delegate, CenterApi, CenterLifeCycle, ConfWriterError, LeadershipStatus, ListOptions, ListResult,
    ResourceKey,
};
use crate::conf_mgr::sync_runtime::ShutdownHandle;
use crate::conf_mgr::ControllerRuntime;
use crate::conf_sync::conf_server::ConfigSyncServer;
use edgion_common::link_sys_core::providers::etcd::EtcdLinkClient;

/// EtcdCenter — configuration center backed by etcd.
pub struct EtcdCenter {
    controller_name: String,
    config: EtcdConfig,
    writer: EtcdStorage,
    client: Arc<EtcdLinkClient>,
    /// Leader election supervisor. Wrapped in `Arc` so the spawned `run` loop and
    /// the `handle`/`gate` accessors can share one instance (spec §4).
    election: Arc<EtcdLeaderElection>,
    /// Per-leadership-term conf_sync stream cancellation slot (spec §6.1). The
    /// election installs a fresh token at each campaign win and cancels it at
    /// demotion; the conf_sync gRPC server captures the current token at stream
    /// start so a demoted leader's streams are actively closed.
    ///
    /// The initial value is a fresh token rather than the process shutdown token
    /// (spec §10.5 S2's literal wording): the process token is not known at
    /// construction time. Safe because every stream ALSO selects on the process
    /// token independently, and no conf_sync stream can exist before the first
    /// campaign win anyway (a follower has no ConfigSyncServer, so all four
    /// RPCs answer UNAVAILABLE before a token is ever captured).
    term_token_slot: Arc<ArcSwap<CancellationToken>>,
    base: CenterBase,
}

impl EtcdCenter {
    /// Create a new EtcdCenter.
    ///
    /// This connects to etcd and initializes the client.
    pub async fn new(config: EtcdConfig) -> Result<Self> {
        config.validate()?;
        let ready_deadline = Duration::from_secs(config.cache_ready_deadline_secs);
        let controller_name = crate::config::conf_center::controller_identity::require_controller_name(
            config.controller_name.as_deref(),
        )?;
        tracing::info!(
            component = "etcd_center",
            endpoints = ?config.endpoints,
            key_prefix = %config.key_prefix,
            "Creating EtcdCenter"
        );

        // Trusted local credentials and disk PEM use the shared TLS resolver
        // through an explicit bootstrap boundary, without resource authority.
        let client = EtcdLinkClient::from_bootstrap("conf-center", config.to_bootstrap_config()?)?;
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
            controller_name,
            config,
            writer,
            client,
            election,
            term_token_slot: Arc::new(ArcSwap::from_pointee(CancellationToken::new())),
            base: CenterBase::new(ready_deadline),
        })
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/etcd/center_test/identity_tests.rs"]
mod identity_tests;

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

        // Spawn the election supervisor. It shares `LeaderState` with the gate
        // injected above and with the lifecycle handle passed below, and installs
        // a fresh token into `term_token_slot` at each campaign win (the conf_sync
        // gRPC server reads the slot via `CenterLifeCycle::term_token_slot`).
        let election = self.election.clone();
        let controller_name = self.controller_name.clone();
        let election_shutdown = shutdown_handle.signal();
        let election_slot = self.term_token_slot.clone();
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
                move |shutdown, runtime, error_tx, scope| {
                    let ctrl = EtcdController::new(
                        client.clone(),
                        key_prefix.clone(),
                        Some(gate.clone()),
                        controller_name.clone(),
                    );
                    tokio::spawn(async move {
                        if let Err(e) = ctrl.run(shutdown, runtime, scope).await {
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

    fn term_token_slot(&self) -> Option<Arc<ArcSwap<CancellationToken>>> {
        Some(self.term_token_slot.clone())
    }

    /// The election (and therefore the signal) is built in `EtcdCenter::new`,
    /// so this returns `Some` from construction onward — there is no window in
    /// which the federation supervisor could ask before the signal exists.
    fn leader_signal(&self) -> Option<tokio::sync::watch::Receiver<bool>> {
        Some(self.election.handle().subscribe())
    }

    fn leadership_status(&self) -> LeadershipStatus {
        let handle = self.election.handle();
        if handle.is_leader() {
            LeadershipStatus::Leader
        } else {
            LeadershipStatus::Follower {
                leader_admin_addr: handle.current_leader().map(|l| l.admin_addr),
            }
        }
    }

    fn runtime(&self) -> Arc<ControllerRuntime> {
        self.base.runtime()
    }
}
