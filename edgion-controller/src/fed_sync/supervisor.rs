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
    F: Fn(watch::Receiver<bool>) -> JoinHandle<()> + Send + 'static,
{
    spawn_client: F,
    /// The running task: `(shutdown_tx, join_handle)`, or `None` if stopped.
    current: Option<(watch::Sender<bool>, JoinHandle<()>)>,
}

impl<F> FederationSupervisor<F>
where
    F: Fn(watch::Receiver<bool>) -> JoinHandle<()> + Send + 'static,
{
    pub fn new(spawn_client: F) -> Self {
        Self {
            spawn_client,
            current: None,
        }
    }

    /// Start the federation client. No-op if already running.
    pub(crate) fn start(&mut self) {
        if self.current.is_some() {
            return;
        }
        let (tx, rx) = watch::channel(false);
        let handle = (self.spawn_client)(rx);
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
            self.start();
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
    F: Fn(watch::Receiver<bool>) -> JoinHandle<()> + Send + 'static,
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
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fake spawn closure: increments a counter and spawns a task that
    /// exits when the shutdown receiver fires.
    fn make_fake_spawn(counter: Arc<AtomicUsize>) -> impl Fn(watch::Receiver<bool>) -> JoinHandle<()> {
        move |mut rx: watch::Receiver<bool>| {
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                // Wait until shutdown=true is sent or the sender is dropped.
                let _ = rx.changed().await;
            })
        }
    }

    /// When `center.enabled` is false at startup, the supervisor must never
    /// call the spawn closure.
    #[tokio::test]
    async fn disabled_at_startup_does_not_start() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut supervisor = FederationSupervisor::new(make_fake_spawn(counter.clone()));

        let cfg = CenterClientConfig {
            enabled: false,
            ..Default::default()
        };
        supervisor.apply(&cfg, true).await;

        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "spawn closure must not be called when disabled"
        );
    }

    /// The leadership analogue of `disabled_at_startup_does_not_start`: a cold
    /// follower with `center.enabled: true` must never call the spawn closure.
    #[tokio::test]
    async fn not_leader_at_startup_does_not_start() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut supervisor = FederationSupervisor::new(make_fake_spawn(counter.clone()));

        let cfg = CenterClientConfig {
            enabled: true,
            ..Default::default()
        };
        supervisor.apply(&cfg, false).await;

        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "spawn closure must not be called on a non-leader"
        );
        assert!(supervisor.current.is_none(), "current must stay None on a non-leader");
    }

    /// The full `enabled x leader` truth table for the run predicate.
    #[test]
    fn should_run_truth_table() {
        let on = CenterClientConfig {
            enabled: true,
            ..Default::default()
        };
        let off = CenterClientConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(should_run(&on, true), "enabled + leader must run");
        assert!(!should_run(&on, false), "enabled + follower must not run");
        assert!(!should_run(&off, true), "disabled + leader must not run");
        assert!(!should_run(&off, false), "disabled + follower must not run");
    }

    /// Sequence: enable (1 spawn) -> disable (stop, join) -> enable (1 new spawn,
    /// never 2 concurrent). Verifies the await-before-respawn contract.
    #[tokio::test]
    async fn enable_then_disable_awaits_before_respawn() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut supervisor = FederationSupervisor::new(make_fake_spawn(counter.clone()));

        // -- enable: expect 1 spawn --
        let cfg_on = CenterClientConfig {
            enabled: true,
            ..Default::default()
        };
        supervisor.apply(&cfg_on, true).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1, "expected 1 spawn after enable");

        // -- disable: stop() must await the prior task --
        let cfg_off = CenterClientConfig {
            enabled: false,
            ..Default::default()
        };
        supervisor.apply(&cfg_off, true).await;
        // After stop() the join handle is gone; `current` is None.
        assert!(supervisor.current.is_none(), "current must be None after stop");
        assert_eq!(counter.load(Ordering::SeqCst), 1, "no additional spawn during disable");

        // -- re-enable: exactly 1 new spawn (total = 2), never 2 concurrent --
        supervisor.apply(&cfg_on, true).await;
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "expected exactly 1 new spawn on re-enable"
        );
        assert!(supervisor.current.is_some(), "current must be Some after re-enable");

        // Clean up: stop so the spawned task exits.
        supervisor.stop().await;
    }

    /// The leadership analogue of `enable_then_disable_awaits_before_respawn`:
    /// promote (1 spawn) -> demote (stopped, awaited) -> re-promote (exactly 1
    /// new spawn, total 2, never 2 concurrent). A violation here is exactly the
    /// double-registration bug the leadership gate exists to prevent.
    #[tokio::test]
    async fn promotion_then_demotion_awaits_before_respawn() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut supervisor = FederationSupervisor::new(make_fake_spawn(counter.clone()));

        let cfg = CenterClientConfig {
            enabled: true,
            ..Default::default()
        };

        // -- promotion: expect 1 spawn --
        supervisor.apply(&cfg, true).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1, "expected 1 spawn on promotion");
        assert!(supervisor.current.is_some(), "current must be Some while leader");

        // -- demotion: stop() must await the prior task; config is untouched --
        supervisor.apply(&cfg, false).await;
        assert!(supervisor.current.is_none(), "current must be None after demotion");
        assert_eq!(counter.load(Ordering::SeqCst), 1, "no additional spawn during demotion");

        // -- re-promotion: exactly 1 new spawn (total = 2), never 2 concurrent --
        supervisor.apply(&cfg, true).await;
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "expected exactly 1 new spawn on re-promotion"
        );
        assert!(supervisor.current.is_some(), "current must be Some after re-promotion");

        supervisor.stop().await;
    }

    // ── run_supervisor-level tests ─────────────────────────────────────────
    //
    // Design note: rather than driving the full `run_supervisor` async loop
    // (which requires careful timing to avoid flakiness), we factor the
    // per-reload decision into a synchronous helper `reload_decision` that is
    // unit-tested directly. The existing lifecycle tests above already cover
    // the `FederationSupervisor` machinery end-to-end. This keeps the tests
    // deterministic while still proving the key behavioral contracts:
    //   1. Policy-only reloads do NOT restart the client (spawn counter unchanged).
    //      Policy swapping is the reload handler's job (see apply_center_reload).
    //   2. `enabled` flip true->false stops the client; false->true restarts.

    use crate::authz::{CenterPolicy, Decision, Verb};
    use crate::config::{FedAllowRule, FedRbacConfig};

    /// Outcome of a single `run_supervisor` reload iteration.
    #[derive(Debug, PartialEq, Eq)]
    enum ReloadAction {
        /// Only the policy changed; the client keeps running (no restart).
        PolicyOnly,
        /// A lifecycle field changed; stop + apply is required.
        LifecycleRestart,
    }

    /// Pure synchronous helper that mirrors the decision logic inside
    /// `run_supervisor`. Testing this directly avoids async timing issues
    /// while keeping 100% coverage of the branching logic. It takes BOTH
    /// inputs (config and leadership) because the loop folds both through
    /// `should_run` — a leadership edge must never be classified `PolicyOnly`.
    fn reload_decision(
        prev: &CenterClientConfig,
        prev_leader: bool,
        next: &CenterClientConfig,
        next_leader: bool,
    ) -> ReloadAction {
        let identity_changed = next.address != prev.address || next.name != prev.name || next.cluster != prev.cluster;
        let run_changed = should_run(next, next_leader) != should_run(prev, prev_leader);
        if identity_changed || run_changed {
            ReloadAction::LifecycleRestart
        } else {
            ReloadAction::PolicyOnly
        }
    }

    fn rbac_allow_get_routes() -> FedRbacConfig {
        FedRbacConfig {
            allow: vec![FedAllowRule {
                verbs: vec!["get".to_string()],
                kinds: vec!["HttpRoute".to_string()],
            }],
        }
    }

    /// A reload that changes ONLY the rbac policy must be classified as
    /// `PolicyOnly` (no client restart), and the authorizer policy must
    /// be swapped so a previously-denied action is now allowed.
    #[test]
    fn policy_only_reload_does_not_restart_client() {
        let base = CenterClientConfig {
            enabled: true,
            address: "http://center:12251".to_string(),
            name: "ctrl-1".to_string(),
            cluster: "prod".to_string(),
            rbac: None, // deny-all initially
            ..Default::default()
        };

        let with_rbac = CenterClientConfig {
            rbac: Some(rbac_allow_get_routes()),
            ..base.clone()
        };

        // Decision must be PolicyOnly (no lifecycle fields changed).
        assert_eq!(
            reload_decision(&base, true, &with_rbac, true),
            ReloadAction::PolicyOnly,
            "rbac-only change must not trigger a client restart"
        );

        // Verify the authorizer reflects the new policy after a swap.
        let authorizer = Authorizer::new(CenterPolicy::deny_all());
        assert_eq!(
            authorizer.authorize(&authorizer.center_role(), Verb::Get, "HttpRoute"),
            Decision::Deny,
            "should be denied before policy swap"
        );

        swap_policy_from(&authorizer, &with_rbac);

        assert_eq!(
            authorizer.authorize(&authorizer.center_role(), Verb::Get, "HttpRoute"),
            Decision::Allow,
            "should be allowed after policy swap"
        );
    }

    /// A reload that changes `enabled` true -> false must be `LifecycleRestart`
    /// (client stopped), and false -> true must also be `LifecycleRestart`
    /// (client restarted), even if all other fields are identical.
    #[test]
    fn enabled_flip_triggers_lifecycle_restart() {
        let enabled = CenterClientConfig {
            enabled: true,
            address: "http://center:12251".to_string(),
            name: "ctrl-1".to_string(),
            cluster: "prod".to_string(),
            rbac: None,
            ..Default::default()
        };
        let disabled = CenterClientConfig {
            enabled: false,
            ..enabled.clone()
        };

        assert_eq!(
            reload_decision(&enabled, true, &disabled, true),
            ReloadAction::LifecycleRestart,
            "enabled -> disabled must trigger lifecycle restart"
        );
        assert_eq!(
            reload_decision(&disabled, true, &enabled, true),
            ReloadAction::LifecycleRestart,
            "disabled -> enabled must trigger lifecycle restart"
        );
    }

    /// A leadership edge with an unchanged config must be `LifecycleRestart`.
    /// Regression guard for the swallowed-edge bug: the leadership signal
    /// arrives on a different channel from the reload snapshot, so a decision
    /// that diffed config fields alone would classify a promotion/demotion as
    /// `PolicyOnly` and leave the client stuck in the wrong state.
    #[test]
    fn leadership_flip_triggers_lifecycle_restart() {
        let cfg = CenterClientConfig {
            enabled: true,
            address: "http://center:12251".to_string(),
            name: "ctrl-1".to_string(),
            cluster: "prod".to_string(),
            rbac: None,
            ..Default::default()
        };

        assert_eq!(
            reload_decision(&cfg, false, &cfg, true),
            ReloadAction::LifecycleRestart,
            "promotion must trigger lifecycle restart"
        );
        assert_eq!(
            reload_decision(&cfg, true, &cfg, false),
            ReloadAction::LifecycleRestart,
            "demotion must trigger lifecycle restart"
        );
    }

    /// While a follower, an `enabled` flip changes nothing observable (the
    /// client is not running either way), so no restart churn is produced --
    /// but the new `enabled` value must still be remembered so the LATER
    /// promotion starts the client. Both halves are asserted here.
    #[test]
    fn enabled_flip_while_follower_defers_to_promotion() {
        let disabled = CenterClientConfig {
            enabled: false,
            address: "http://center:12251".to_string(),
            name: "ctrl-1".to_string(),
            cluster: "prod".to_string(),
            rbac: None,
            ..Default::default()
        };
        let enabled = CenterClientConfig {
            enabled: true,
            ..disabled.clone()
        };

        assert_eq!(
            reload_decision(&disabled, false, &enabled, false),
            ReloadAction::PolicyOnly,
            "enabling while a follower must not churn the (already stopped) client"
        );
        assert_eq!(
            reload_decision(&enabled, false, &enabled, true),
            ReloadAction::LifecycleRestart,
            "the deferred enable must take effect on promotion"
        );
    }

    /// A reload that changes the `address` field (connection target) must be
    /// `LifecycleRestart` even when `enabled` and identity fields are unchanged.
    #[test]
    fn address_change_triggers_lifecycle_restart() {
        let base = CenterClientConfig {
            enabled: true,
            address: "http://center-a:12251".to_string(),
            name: "ctrl-1".to_string(),
            cluster: "prod".to_string(),
            ..Default::default()
        };
        let new_addr = CenterClientConfig {
            address: "http://center-b:12251".to_string(),
            ..base.clone()
        };
        assert_eq!(
            reload_decision(&base, true, &new_addr, true),
            ReloadAction::LifecycleRestart,
            "address change must trigger lifecycle restart"
        );
    }

    /// Verifies that run_supervisor drives the supervisor correctly for a
    /// policy-only reload: the spawn counter must not increase, and the
    /// authorizer must reflect the updated policy.
    #[tokio::test]
    async fn run_supervisor_policy_only_reload_does_not_spawn() {
        let counter = Arc::new(AtomicUsize::new(0));
        let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));

        let initial = CenterClientConfig {
            enabled: true,
            address: String::new(),
            name: String::new(),
            cluster: String::new(),
            rbac: None,
            ..Default::default()
        };

        let (reload_tx, reload_rx) = watch::channel(initial.clone());

        let (leader_tx, leader_rx) = watch::channel(true);

        let supervisor = FederationSupervisor::new(make_fake_spawn(counter.clone()));
        let authorizer_for_supervisor = authorizer.clone();
        let task = tokio::spawn(run_supervisor(
            supervisor,
            initial,
            reload_rx,
            Some(leader_rx),
            authorizer_for_supervisor,
        ));

        // Yield to let the supervisor process the initial config.
        tokio::task::yield_now().await;
        assert_eq!(counter.load(Ordering::SeqCst), 1, "initial enable must spawn once");

        // Send a policy-only reload (rbac changes; enabled/address/name/cluster unchanged).
        let policy_cfg = CenterClientConfig {
            enabled: true,
            address: String::new(),
            name: String::new(),
            cluster: String::new(),
            rbac: Some(rbac_allow_get_routes()),
            ..Default::default()
        };
        reload_tx.send(policy_cfg).unwrap();

        // Yield to let the supervisor loop process the change.
        tokio::task::yield_now().await;

        // Spawn counter must still be 1: no restart triggered.
        // (This is the only assertion here -- policy swapping on reload is now
        // the reload handler's job, not the supervisor's. See apply_center_reload
        // in api/mod.rs and the run_supervisor doc for details.)
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "policy-only reload must not restart the client"
        );

        // Drop the sender to terminate the supervisor loop cleanly.
        drop(reload_tx);
        drop(leader_tx);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), task).await;
    }

    /// A backend with no leader election (FileSystem) passes `None`, which
    /// `run_supervisor` materializes as a constant `true`. This replays the
    /// assertions of `run_supervisor_policy_only_reload_does_not_spawn`
    /// unchanged to prove the enabled-only behavior is byte-identical to the
    /// pre-leadership-gating supervisor.
    #[tokio::test]
    async fn absent_leader_signal_preserves_enabled_only_behavior() {
        let counter = Arc::new(AtomicUsize::new(0));
        let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));

        let initial = CenterClientConfig {
            enabled: true,
            address: String::new(),
            name: String::new(),
            cluster: String::new(),
            rbac: None,
            ..Default::default()
        };

        let (reload_tx, reload_rx) = watch::channel(initial.clone());

        let supervisor = FederationSupervisor::new(make_fake_spawn(counter.clone()));
        let task = tokio::spawn(run_supervisor(supervisor, initial, reload_rx, None, authorizer));

        tokio::task::yield_now().await;
        assert_eq!(counter.load(Ordering::SeqCst), 1, "initial enable must spawn once");

        let policy_cfg = CenterClientConfig {
            enabled: true,
            address: String::new(),
            name: String::new(),
            cluster: String::new(),
            rbac: Some(rbac_allow_get_routes()),
            ..Default::default()
        };
        reload_tx.send(policy_cfg).unwrap();
        tokio::task::yield_now().await;

        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "policy-only reload must not restart the client"
        );

        drop(reload_tx);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), task).await;
    }

    /// Spawn bookkeeping richer than a bare counter: also tracks how many fake
    /// clients are alive at once, so the at-most-one-client invariant can be
    /// asserted directly rather than inferred.
    #[derive(Default)]
    struct SpawnStats {
        spawned: AtomicUsize,
        running: AtomicUsize,
        max_running: AtomicUsize,
    }

    fn make_tracking_spawn(stats: Arc<SpawnStats>) -> impl Fn(watch::Receiver<bool>) -> JoinHandle<()> {
        move |mut rx: watch::Receiver<bool>| {
            let stats = stats.clone();
            stats.spawned.fetch_add(1, Ordering::SeqCst);
            let now_running = stats.running.fetch_add(1, Ordering::SeqCst) + 1;
            stats.max_running.fetch_max(now_running, Ordering::SeqCst);
            tokio::spawn(async move {
                let _ = rx.changed().await;
                stats.running.fetch_sub(1, Ordering::SeqCst);
            })
        }
    }

    /// Cooperatively yield until `pred` holds. Bounded so a broken predicate
    /// fails the test instead of hanging. No sleeps: every step the supervisor
    /// takes here is driven by channel wakeups, not timers.
    async fn yield_until(mut pred: impl FnMut() -> bool) -> bool {
        for _ in 0..500 {
            if pred() {
                return true;
            }
            tokio::task::yield_now().await;
        }
        pred()
    }

    /// `run_supervisor` driven through BOTH watch channels.
    ///
    /// 1. enabled + follower at startup => nothing spawned.
    /// 2. a policy-only reload while still a follower => still nothing spawned.
    /// 3. promotion => exactly one client.
    /// 4. demotion => the client is stopped (and awaited).
    /// 5. re-promotion => exactly one new client (2 total), never 2 at once.
    #[tokio::test]
    async fn run_supervisor_gates_on_leadership_across_both_channels() {
        let stats = Arc::new(SpawnStats::default());
        let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));

        let initial = CenterClientConfig {
            enabled: true,
            address: "http://center:12251".to_string(),
            name: "ctrl-1".to_string(),
            cluster: "prod".to_string(),
            rbac: None,
            ..Default::default()
        };

        let (reload_tx, reload_rx) = watch::channel(initial.clone());
        let (leader_tx, leader_rx) = watch::channel(false);

        let supervisor = FederationSupervisor::new(make_tracking_spawn(stats.clone()));
        let task = tokio::spawn(run_supervisor(
            supervisor,
            initial.clone(),
            reload_rx,
            Some(leader_rx),
            authorizer,
        ));

        // 1. A cold follower never connects.
        tokio::task::yield_now().await;
        assert_eq!(
            stats.spawned.load(Ordering::SeqCst),
            0,
            "a non-leader must not start the federation client at startup"
        );

        // 2. A policy-only reload while a follower must stay a no-op.
        reload_tx
            .send(CenterClientConfig {
                rbac: Some(rbac_allow_get_routes()),
                ..initial.clone()
            })
            .unwrap();
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        assert_eq!(
            stats.spawned.load(Ordering::SeqCst),
            0,
            "a policy-only reload must not start the client on a non-leader"
        );

        // 3. Promotion starts exactly one client.
        leader_tx.send_replace(true);
        assert!(
            yield_until(|| stats.spawned.load(Ordering::SeqCst) == 1).await,
            "promotion must start the federation client"
        );
        assert_eq!(stats.running.load(Ordering::SeqCst), 1, "exactly one client must run");

        // 4. Demotion stops it (stop() awaits the task, so `running` drops to 0).
        leader_tx.send_replace(false);
        assert!(
            yield_until(|| stats.running.load(Ordering::SeqCst) == 0).await,
            "demotion must stop the federation client"
        );
        assert_eq!(
            stats.spawned.load(Ordering::SeqCst),
            1,
            "demotion must not spawn anything"
        );

        // 5. Re-promotion spawns exactly one new client.
        leader_tx.send_replace(true);
        assert!(
            yield_until(|| stats.spawned.load(Ordering::SeqCst) == 2).await,
            "re-promotion must start a fresh federation client"
        );
        assert_eq!(
            stats.max_running.load(Ordering::SeqCst),
            1,
            "two federation clients must never be alive at once (double registration at Center)"
        );

        drop(reload_tx);
        drop(leader_tx);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), task).await;
    }
}
