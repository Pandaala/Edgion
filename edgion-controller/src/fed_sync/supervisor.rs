//! Federation supervisor: owns the fed client lifecycle for fixed startup
//! configuration. Only the leader replica holds the federation session;
//! configuration changes require a Controller process restart.
//!
//! The run predicate is [`should_run`] — `cfg.enabled && is_leader`. The
//! leadership half exists because the federation session at Center is keyed by
//! `controller_id` (`cluster/name`), which is IDENTICAL on every Controller
//! replica: a second replica registering displaces and cancels the first, so
//! two ungated replicas take the session from each other forever, each takeover
//! minting a new `server_id` (Center re-establishes watches, a full StatsReport
//! snapshot is pushed, and the Gateway re-lists).
//!
//! Stop signals the client's one-way `watch` shutdown channel and AWAITS the
//! prior task (bounded timeout) before respawning. This await-before-respawn
//! prevents two clients from double-registering at Center simultaneously, and
//! applies to leadership changes and process shutdown.
//!
//! `FederationSupervisor` is generic over the spawn closure so unit tests can
//! substitute a fake client without touching I/O.

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::conf_mgr::sync_runtime::ShutdownSignal;
use crate::config::CenterClientConfig;
use edgion_common::observe::fed_metrics;

/// Maximum time to wait for the federation client task to exit on stop.
const STOP_TIMEOUT_SECS: u64 = 10;

/// The federation client's run predicate: enabled by the operator AND this
/// process is the leader.
///
/// Pure and synchronous so the full `enabled x leader` truth table is unit
/// testable without driving the async loop. Backends with no leader election
/// (FileSystem) pass `is_leader = true` unconditionally — see `run_supervisor`.
pub(crate) fn should_run(cfg: &CenterClientConfig, is_leader: bool) -> bool {
    cfg.enabled && is_leader
}

/// Owns the lifecycle of exactly zero or one federation client task.
///
/// Generic over `F` so unit tests can inject a fake spawn closure.
pub struct FederationSupervisor<F>
where
    F: Fn(CenterClientConfig, watch::Receiver<bool>) -> JoinHandle<()> + Send + 'static,
{
    spawn_client: F,
    /// The running task: `(shutdown_tx, join_handle)`, or `None` if stopped.
    current: Option<(watch::Sender<bool>, JoinHandle<()>)>,
}

impl<F> FederationSupervisor<F>
where
    F: Fn(CenterClientConfig, watch::Receiver<bool>) -> JoinHandle<()> + Send + 'static,
{
    pub fn new(spawn_client: F) -> Self {
        Self {
            spawn_client,
            current: None,
        }
    }

    /// Start the federation client. No-op if already running.
    pub(crate) fn start(&mut self, cfg: &CenterClientConfig) {
        if self.current.is_some() {
            return;
        }
        let (tx, rx) = watch::channel(false);
        let handle = (self.spawn_client)(cfg.clone(), rx);
        self.current = Some((tx, handle));
        tracing::info!(component = "fed_supervisor", "federation client started");
    }

    /// Stop the federation client and AWAIT the task before returning.
    ///
    /// The bounded timeout prevents an unresponsive client from blocking
    /// the supervisor loop indefinitely.
    ///
    /// # Timeout behavior
    ///
    /// If the federation client task does not exit within `STOP_TIMEOUT_SECS`,
    /// `stop()` returns anyway and the old task continues running detached.
    /// A subsequent `start()` will spawn a new client, which may briefly
    /// overlap with the still-running detached task. This is intentional for a
    /// kill-switch scenario: forward progress (starting the new client) takes
    /// priority over strict serial ordering. As a result, the at-most-one-client
    /// invariant is best-effort under extreme failure (hung task).
    pub(crate) async fn stop(&mut self) {
        if let Some((tx, handle)) = self.current.take() {
            // Signal the client to shut down (one-way).
            let _ = tx.send(true);
            // Wait for the task to finish (bounded).
            let _ = tokio::time::timeout(std::time::Duration::from_secs(STOP_TIMEOUT_SECS), handle).await;
            tracing::info!(component = "fed_supervisor", "federation client stopped");
        }
    }

    /// Apply startup configuration plus the current leadership state: start if
    /// [`should_run`], stop otherwise.
    ///
    /// Callers do not need to call `stop()` before `apply()`; demotion
    /// stops the client before a subsequent promotion can start another.
    ///
    /// Metrics are published here (not in `start`/`stop`) so the two gauges are
    /// always emitted as a coherent pair: `kill_switch_state` reports only the
    /// operator's intent, `client_running` reports the derived reality. A
    /// follower therefore reports `kill_switch_state=1, client_running=0`
    /// instead of falsely claiming the operator engaged the kill switch.
    pub(crate) async fn apply(&mut self, cfg: &CenterClientConfig, is_leader: bool) {
        if should_run(cfg, is_leader) {
            self.start(cfg);
        } else {
            self.stop().await;
        }
        fed_metrics::record_kill_switch_state(cfg.enabled);
        fed_metrics::record_client_running(self.current.is_some());
    }
}

/// Run the fixed-config federation client while this Controller is leader.
///
/// Startup installs the authorization policy before this task is spawned.
/// Configuration changes require a process restart. A backend without leader
/// election supplies `None` and behaves as an unconditional leader.
/// Unexpected client exit still requires operator recovery by process restart;
/// this loop does not introduce automatic client restart.
pub async fn run_supervisor<F>(
    mut supervisor: FederationSupervisor<F>,
    config: CenterClientConfig,
    leader_signal: Option<watch::Receiver<bool>>,
    mut shutdown: ShutdownSignal,
) where
    F: Fn(CenterClientConfig, watch::Receiver<bool>) -> JoinHandle<()> + Send + 'static,
{
    if shutdown.is_shutdown() {
        return;
    }
    // Retain the constant sender so a backend without elections cannot spin
    // on a closed watch channel.
    let (_constant_leader_tx, mut leader_rx) = match leader_signal {
        Some(rx) => (None, rx),
        None => {
            let (tx, rx) = watch::channel(true);
            (Some(tx), rx)
        }
    };
    let initial_leader = *leader_rx.borrow_and_update();
    supervisor.apply(&config, initial_leader).await;
    let mut leader_alive = true;

    loop {
        tokio::select! {
            biased;
            _ = shutdown.wait() => {
                supervisor.stop().await;
                break;
            }
            changed = leader_rx.changed(), if leader_alive => {
                if changed.is_err() {
                    leader_alive = false;
                    continue;
                }
                let is_leader = *leader_rx.borrow_and_update();
                if !shutdown.is_shutdown() {
                    supervisor.apply(&config, is_leader).await;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/fed_sync/supervisor_test.rs"]
mod tests;
