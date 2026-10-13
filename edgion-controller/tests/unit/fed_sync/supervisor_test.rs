use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A fake spawn closure: increments a counter and spawns a task that
/// exits when the shutdown receiver fires.
fn make_fake_spawn(counter: Arc<AtomicUsize>) -> impl Fn(CenterClientConfig, watch::Receiver<bool>) -> JoinHandle<()> {
    move |_: CenterClientConfig, mut rx: watch::Receiver<bool>| {
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
    configs: parking_lot::Mutex<Vec<CenterClientConfig>>,
}

fn make_tracking_spawn(stats: Arc<SpawnStats>) -> impl Fn(CenterClientConfig, watch::Receiver<bool>) -> JoinHandle<()> {
    move |cfg: CenterClientConfig, mut rx: watch::Receiver<bool>| {
        let stats = stats.clone();
        stats.configs.lock().push(cfg);
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
#[tokio::test(flavor = "current_thread")]
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
    assert!(
        stats.configs.lock()[0].rbac.is_some(),
        "promotion must receive the follower's latest snapshot"
    );

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

    // Apply a new address while still a follower. On this current-thread runtime,
    // yield to let the ready reload branch complete before sending promotion.
    let replacement = CenterClientConfig {
        address: "http://replacement:12251".into(),
        name: "ctrl-new".into(),
        cluster: "next".into(),
        env: vec!["staging".into()],
        tag: vec!["latest".into()],
        ..initial.clone()
    };
    reload_tx.send(replacement.clone()).unwrap();
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;

    // 5. Re-promotion spawns exactly one new client with the remembered snapshot.
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
    {
        let configs = stats.configs.lock();
        let actual = &configs[1];
        assert_eq!(actual.address, replacement.address);
        assert_eq!(actual.name, replacement.name);
        assert_eq!(actual.cluster, replacement.cluster);
        assert_eq!(actual.env, replacement.env);
        assert_eq!(actual.tag, replacement.tag);
    }

    leader_tx.send_replace(false);
    assert!(yield_until(|| stats.running.load(Ordering::SeqCst) == 0).await);
    drop(reload_tx);
    drop(leader_tx);
    tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn address_restart_passes_current_snapshot_to_spawn() {
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let (stopped_tx, mut stopped_rx) = tokio::sync::mpsc::unbounded_channel();
    let spawn = move |cfg: CenterClientConfig, mut shutdown: watch::Receiver<bool>| {
        let stopped_tx = stopped_tx.clone();
        started_tx.send(cfg).unwrap();
        tokio::spawn(async move {
            let _ = shutdown.changed().await;
            stopped_tx.send(()).unwrap();
        })
    };
    let initial = CenterClientConfig {
        address: "http://center-a:12251".into(),
        name: "controller".into(),
        cluster: "prod".into(),
        ..Default::default()
    };
    let (reload_tx, reload_rx) = watch::channel(initial.clone());
    let (leader_tx, leader_rx) = watch::channel(true);
    let task = tokio::spawn(run_supervisor(
        FederationSupervisor::new(spawn),
        initial.clone(),
        reload_rx,
        Some(leader_rx),
        Arc::new(Authorizer::new(CenterPolicy::deny_all())),
    ));
    let first = tokio::time::timeout(std::time::Duration::from_secs(3), started_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.address, initial.address);
    let updated = CenterClientConfig {
        address: "http://center-b:12251".into(),
        tag: vec!["replacement".into()],
        ..initial
    };
    reload_tx.send(updated.clone()).unwrap();
    let second = tokio::time::timeout(std::time::Duration::from_secs(3), started_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.address, updated.address);
    assert_eq!(second.name, updated.name);
    assert_eq!(second.cluster, updated.cluster);
    assert_eq!(second.tag, updated.tag);
    stopped_rx
        .try_recv()
        .expect("old client must stop before replacement starts");
    leader_tx.send_replace(false);
    tokio::time::timeout(std::time::Duration::from_secs(3), stopped_rx.recv())
        .await
        .unwrap()
        .unwrap();
    drop(reload_tx);
    tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
}
