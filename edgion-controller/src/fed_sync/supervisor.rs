//! Federation supervisor: owns the fed client lifecycle so `center.enabled` can
//! be toggled at runtime without restarting the Controller, and so only the
//! leader replica ever holds the federation session.
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
//! applies to leadership edges exactly as it does to config edges.
//!
//! `FederationSupervisor` is generic over the spawn closure so unit tests can
//! substitute a fake client without touching I/O.

use std::sync::Arc;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::authz::Authorizer;
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

    /// Apply a new config snapshot plus the current leadership state: start if
    /// [`should_run`], stop otherwise.
    ///
    /// `run_supervisor` handles lifecycle transitions explicitly (stop then
    /// apply on connection-identity changes). Callers do not need to call
    /// `stop()` before `apply()` — the supervisor decides when that is needed.
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

/// Drive the supervisor from a reload watch channel and a leadership watch
/// channel, plus the initial config.
///
/// This function runs for the lifetime of the process (until the
/// `reload_rx` sender is dropped). It applies the initial policy and
/// lifecycle state, then loops on config and leadership changes.
///
/// # Leadership gating
///
/// `leader_rx` is `ConfMgr::leader_signal()`. `None` means the backend has no
/// leader election at all (FileSystem); it is materialized as a constant `true`
/// channel so that path behaves byte-identically to the pre-gating supervisor.
/// The signal is deliberately NOT `leadership_status()` — that enum drives the
/// Admin API 409 gate and stays `NotApplicable` in Kubernetes, where this signal
/// is nevertheless `Some`. See `CenterLifeCycle::leader_signal`.
///
/// # Reload strategy
///
/// Two categories of change are handled differently:
///
/// * **Policy-only change** (`rbac` updated, but `enabled` / `address` /
///   `name` / `cluster` unchanged, and leadership unchanged): the reload handler
///   already swapped the policy via `ArcSwap` before notifying this channel. The
///   supervisor only checks for lifecycle changes and, finding none, does
///   nothing. The running client keeps its connection; the next request it
///   authorizes uses the new policy that is already live.
///
/// * **Lifecycle change** (`address` / `name` / `cluster` changed, or
///   [`should_run`] flipped because `enabled` or leadership moved): the old
///   client is stopped first (awaited, bounded timeout), then a fresh client is
///   started according to the new predicate. This prevents two clients from
///   double-registering at Center and ensures the new connection uses the
///   updated identity / address.
///
///   The edge test compares `should_run` across the FULL input pair rather than
///   diffing config fields: a leadership edge arrives on the other channel and
///   would otherwise be swallowed as a "no config change" no-op, and an
///   `enabled: false -> true` received while a follower must not be forgotten
///   (it has to take effect on the later promotion).
///
/// # Single writer for the reload path
///
/// `apply_center_reload` (the `/api/v1/reload` handler) is the SOLE policy
/// writer on the reload path: it swaps the policy eagerly before sending on
/// this channel. The supervisor owns lifecycle (start / stop / restart) only
/// and must NOT call `swap_policy_from` inside the loop, because doing so
/// would create two unordered writers on the same `ArcSwap` and could
/// overwrite a newer policy with a stale one (transient wrong-policy window).
///
/// # Intermediate reload states
///
/// The watch channel keeps only the latest value; intermediate states are
/// coalesced and silently dropped. This is intentional: the supervisor
/// converges to the final state, not every transient snapshot. Callers
/// must not assume every reload notification is acted upon in order.
///
/// # Unexpected client exit
///
/// If `fed_client::run` ends unexpectedly (e.g. panic), the supervisor does
/// NOT auto-restart it until the next lifecycle reload. `run` is written to
/// be panic-resistant (returns on all fatal conditions rather than panicking),
/// so unexpected exits are expected to be rare. This is a known limitation.
pub async fn run_supervisor<F>(
    mut supervisor: FederationSupervisor<F>,
    initial: CenterClientConfig,
    mut reload_rx: watch::Receiver<CenterClientConfig>,
    leader_signal: Option<watch::Receiver<bool>>,
    authorizer: Arc<Authorizer>,
) where
    F: Fn(CenterClientConfig, watch::Receiver<bool>) -> JoinHandle<()> + Send + 'static,
{
    // Apply the initial policy at startup. This is UNCONDITIONAL — a follower
    // still needs the correct Center RBAC policy resident so promotion is
    // instantaneous and never serves a stale policy for a window.
    // No competing writer exists here: the admin server is not yet accepting
    // requests, so the reload handler cannot race this call.
    swap_policy_from(&authorizer, &initial);

    // Materialize an absent signal (FileSystem: no election) as a constant
    // `true` channel. The sender is held for the whole loop so `changed()`
    // never resolves — without it the select arm would spin on a closed
    // channel.
    let (_constant_leader_tx, mut leader_rx) = match leader_signal {
        Some(rx) => (None, rx),
        None => {
            let (tx, rx) = watch::channel(true);
            (Some(tx), rx)
        }
    };

    let mut prev_leader = *leader_rx.borrow_and_update();
    supervisor.apply(&initial, prev_leader).await;

    // Track the previous snapshot to detect lifecycle-affecting changes.
    let mut prev = initial;
    // A closed leadership channel would make `changed()` resolve immediately
    // forever; disable that select arm instead of spinning.
    let mut leader_alive = true;

    loop {
        let mut cfg = prev.clone();
        let mut is_leader = prev_leader;

        tokio::select! {
            changed = reload_rx.changed() => {
                if changed.is_err() {
                    // The reload sender is gone: the process is shutting down.
                    break;
                }
                cfg = reload_rx.borrow().clone();
            }
            changed = leader_rx.changed(), if leader_alive => {
                if changed.is_err() {
                    leader_alive = false;
                    continue;
                }
                is_leader = *leader_rx.borrow();
            }
        }

        // Do NOT call swap_policy_from here. The reload handler
        // (apply_center_reload in api/mod.rs) is the single policy writer on
        // the reload path: it swaps the policy before sending on this channel.
        // Calling swap_policy_from here would create a second, unordered writer
        // that could overwrite a newer policy with a stale snapshot.

        // Determine whether the connection identity changed, or whether the
        // run predicate flipped. `should_run` folds BOTH inputs, so a
        // leadership edge arriving on the other channel is never swallowed as
        // a "no config change" no-op.
        let identity_changed = cfg.address != prev.address || cfg.name != prev.name || cfg.cluster != prev.cluster;
        let run_changed = should_run(&cfg, is_leader) != should_run(&prev, prev_leader);

        if identity_changed || run_changed {
            // Stop the old client (awaited, bounded timeout) before starting
            // a new one to prevent dual registration at Center. This holds for
            // a leadership edge exactly as for a config edge.
            supervisor.stop().await;
            supervisor.apply(&cfg, is_leader).await;
        }
        // If only the policy changed, the client keeps running; the reload
        // handler already made the new policy live before notifying us.

        prev = cfg;
        prev_leader = is_leader;
    }
}

/// Swap the authorizer's Center policy from a config snapshot.
///
/// `None` rbac => built-in default policy (read-all-except-Secret +
/// EdgionConfigData write + RegionRoute failover). `Some(r)` => operator's
/// explicit config used verbatim.
fn swap_policy_from(authorizer: &Authorizer, cfg: &CenterClientConfig) {
    let policy = crate::fed_sync::default_policy::center_policy_from(cfg.rbac.as_ref());
    authorizer.swap_policy(policy);
}

#[cfg(test)]
#[path = "../../tests/unit/fed_sync/supervisor_test.rs"]
mod tests;
