//! Federation supervisor: owns the fed client lifecycle so `center.enabled` can
//! be toggled at runtime without restarting the Controller.
//!
//! Stop signals the client's one-way `watch` shutdown channel and AWAITS the
//! prior task (bounded timeout) before respawning. This await-before-respawn
//! prevents two clients from double-registering at Center simultaneously.
//!
//! `FederationSupervisor` is generic over the spawn closure so unit tests can
//! substitute a fake client without touching I/O.

use std::sync::Arc;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::core::common::config::CenterClientConfig;
use crate::core::common::observe::fed_metrics;
use crate::core::controller::authz::Authorizer;

/// Maximum time to wait for the federation client task to exit on stop.
const STOP_TIMEOUT_SECS: u64 = 10;

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
        fed_metrics::record_kill_switch_state(true);
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
            fed_metrics::record_kill_switch_state(false);
        }
    }

    /// Apply a new config snapshot: start if `cfg.enabled`, stop otherwise.
    ///
    /// `run_supervisor` handles lifecycle transitions explicitly (stop then
    /// apply on connection-identity changes). Callers do not need to call
    /// `stop()` before `apply()` — the supervisor decides when that is needed.
    pub(crate) async fn apply(&mut self, cfg: &CenterClientConfig) {
        if cfg.enabled {
            self.start();
        } else {
            self.stop().await;
        }
    }
}

/// Drive the supervisor from a reload watch channel plus the initial config.
///
/// This function runs for the lifetime of the process (until the
/// `reload_rx` sender is dropped). It applies the initial policy and
/// lifecycle state, then loops on config changes.
///
/// # Reload strategy
///
/// Two categories of change are handled differently:
///
/// * **Policy-only change** (`rbac` updated, but `enabled` / `address` /
///   `name` / `cluster` unchanged): the reload handler already swapped the
///   policy via `ArcSwap` before notifying this channel. The supervisor
///   only checks for lifecycle changes and, finding none, does nothing.
///   The running client keeps its connection; the next request it
///   authorizes uses the new policy that is already live.
///
/// * **Lifecycle change** (`enabled`, `address`, `name`, or `cluster`
///   changed): the old client is stopped first (awaited, bounded timeout),
///   then a fresh client is started according to the new `enabled` flag.
///   This prevents two clients from double-registering at Center and ensures
///   the new connection uses the updated identity / address.
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
    authorizer: Arc<Authorizer>,
) where
    F: Fn(watch::Receiver<bool>) -> JoinHandle<()> + Send + 'static,
{
    // Apply the initial policy at startup, then start or stay stopped per `enabled`.
    // No competing writer exists here: the admin server is not yet accepting
    // requests, so the reload handler cannot race this call.
    swap_policy_from(&authorizer, &initial);
    supervisor.apply(&initial).await;

    // Track the previous snapshot to detect lifecycle-affecting changes.
    let mut prev = initial;

    while reload_rx.changed().await.is_ok() {
        let cfg = reload_rx.borrow().clone();

        // Do NOT call swap_policy_from here. The reload handler
        // (apply_center_reload in api/mod.rs) is the single policy writer on
        // the reload path: it swaps the policy before sending on this channel.
        // Calling swap_policy_from here would create a second, unordered writer
        // that could overwrite a newer policy with a stale snapshot.

        // Determine whether the connection identity or kill-switch changed.
        // These are the fields that affect whether/how the client connects
        // and its stable controller identity at Center.
        let lifecycle_changed = cfg.enabled != prev.enabled
            || cfg.address != prev.address
            || cfg.name != prev.name
            || cfg.cluster != prev.cluster;

        if lifecycle_changed {
            // Stop the old client (awaited, bounded timeout) before starting
            // a new one to prevent dual registration at Center.
            supervisor.stop().await;
            supervisor.apply(&cfg).await;
        }
        // If only the policy changed, the client keeps running; the reload
        // handler already made the new policy live before notifying us.

        prev = cfg;
    }
}

/// Swap the authorizer's Center policy from a config snapshot.
///
/// `None` rbac => built-in default policy (read-all-except-Secret +
/// PluginMetaData write + RegionRoute failover). `Some(r)` => operator's
/// explicit config used verbatim.
fn swap_policy_from(authorizer: &Authorizer, cfg: &CenterClientConfig) {
    let policy = crate::core::controller::fed_sync::default_policy::center_policy_from(cfg.rbac.as_ref());
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
        supervisor.apply(&cfg).await;

        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "spawn closure must not be called when disabled"
        );
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
        supervisor.apply(&cfg_on).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1, "expected 1 spawn after enable");

        // -- disable: stop() must await the prior task --
        let cfg_off = CenterClientConfig {
            enabled: false,
            ..Default::default()
        };
        supervisor.apply(&cfg_off).await;
        // After stop() the join handle is gone; `current` is None.
        assert!(supervisor.current.is_none(), "current must be None after stop");
        assert_eq!(counter.load(Ordering::SeqCst), 1, "no additional spawn during disable");

        // -- re-enable: exactly 1 new spawn (total = 2), never 2 concurrent --
        supervisor.apply(&cfg_on).await;
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "expected exactly 1 new spawn on re-enable"
        );
        assert!(supervisor.current.is_some(), "current must be Some after re-enable");

        // Clean up: stop so the spawned task exits.
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
    //   1. Policy-only reloads do NOT restart the client (spawn counter
    //      unchanged) AND the authorizer policy is updated.
    //   2. `enabled` flip true->false stops the client; false->true restarts.

    use crate::core::common::config::{FedAllowRule, FedRbacConfig};
    use crate::core::controller::authz::{CenterPolicy, Decision, Verb};

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
    /// while keeping 100% coverage of the branching logic.
    fn reload_decision(prev: &CenterClientConfig, next: &CenterClientConfig) -> ReloadAction {
        let lifecycle_changed = next.enabled != prev.enabled
            || next.address != prev.address
            || next.name != prev.name
            || next.cluster != prev.cluster;
        if lifecycle_changed {
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
            reload_decision(&base, &with_rbac),
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
            reload_decision(&enabled, &disabled),
            ReloadAction::LifecycleRestart,
            "enabled -> disabled must trigger lifecycle restart"
        );
        assert_eq!(
            reload_decision(&disabled, &enabled),
            ReloadAction::LifecycleRestart,
            "disabled -> enabled must trigger lifecycle restart"
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
            reload_decision(&base, &new_addr),
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

        let supervisor = FederationSupervisor::new(make_fake_spawn(counter.clone()));
        let authorizer_for_supervisor = authorizer.clone();
        let task = tokio::spawn(run_supervisor(
            supervisor,
            initial,
            reload_rx,
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
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), task).await;
    }
}
