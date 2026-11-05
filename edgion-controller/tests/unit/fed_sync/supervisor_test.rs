use super::*;
use crate::conf_mgr::sync_runtime::ShutdownHandle;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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

/// Promote (1 spawn) -> demote (stopped, awaited) -> re-promote (exactly 1
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

// Drive the real supervisor loop; notifications acknowledge each transition.
fn observed_spawn(
    started: tokio::sync::mpsc::UnboundedSender<CenterClientConfig>,
    stopped: tokio::sync::mpsc::UnboundedSender<()>,
) -> impl Fn(CenterClientConfig, watch::Receiver<bool>) -> JoinHandle<()> {
    move |cfg, mut shutdown| {
        started.send(cfg).unwrap();
        let stopped = stopped.clone();
        tokio::spawn(async move {
            let _ = shutdown.changed().await;
            stopped.send(()).unwrap();
        })
    }
}

async fn receive<T>(rx: &mut tokio::sync::mpsc::UnboundedReceiver<T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn run_supervisor_gates_on_leadership_and_stops_on_shutdown() {
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let (stopped_tx, mut stopped_rx) = tokio::sync::mpsc::unbounded_channel();
    let (leader_tx, leader_rx) = watch::channel(false);
    let shutdown = ShutdownHandle::new();
    let config = CenterClientConfig {
        address: "https://center.example:12251".into(),
        name: "controller".into(),
        cluster: "prod".into(),
        ..Default::default()
    };
    let task = tokio::spawn(run_supervisor(
        FederationSupervisor::new(observed_spawn(started_tx, stopped_tx)),
        config.clone(),
        Some(leader_rx),
        shutdown.signal(),
    ));
    tokio::task::yield_now().await;
    assert!(started_rx.try_recv().is_err(), "follower must not connect");
    leader_tx.send_replace(true);
    let first = receive(&mut started_rx).await;
    assert_eq!(first.address, config.address);
    leader_tx.send_replace(false);
    receive(&mut stopped_rx).await;
    leader_tx.send_replace(true);
    let second = receive(&mut started_rx).await;
    assert_eq!(second.address, config.address);
    assert_eq!(second.name, config.name);
    assert_eq!(second.cluster, config.cluster);
    assert!(started_rx.try_recv().is_err(), "one client per promotion");
    shutdown.shutdown();
    receive(&mut stopped_rx).await;
    tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn absent_leader_signal_runs_until_process_shutdown() {
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let (stopped_tx, mut stopped_rx) = tokio::sync::mpsc::unbounded_channel();
    let shutdown = ShutdownHandle::new();
    let task = tokio::spawn(run_supervisor(
        FederationSupervisor::new(observed_spawn(started_tx, stopped_tx)),
        CenterClientConfig::default(),
        None,
        shutdown.signal(),
    ));
    receive(&mut started_rx).await;
    shutdown.shutdown();
    receive(&mut stopped_rx).await;
    tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn already_requested_shutdown_never_starts_client() {
    let counter = Arc::new(AtomicUsize::new(0));
    let shutdown = ShutdownHandle::new();
    shutdown.shutdown();
    run_supervisor(
        FederationSupervisor::new(make_fake_spawn(counter.clone())),
        CenterClientConfig::default(),
        None,
        shutdown.signal(),
    )
    .await;
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn closed_leadership_channel_still_observes_process_shutdown() {
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let (stopped_tx, mut stopped_rx) = tokio::sync::mpsc::unbounded_channel();
    let (leader_tx, leader_rx) = watch::channel(true);
    let shutdown = ShutdownHandle::new();
    let task = tokio::spawn(run_supervisor(
        FederationSupervisor::new(observed_spawn(started_tx, stopped_tx)),
        CenterClientConfig::default(),
        Some(leader_rx),
        shutdown.signal(),
    ));
    receive(&mut started_rx).await;
    drop(leader_tx);
    tokio::task::yield_now().await;
    shutdown.shutdown();
    receive(&mut stopped_rx).await;
    tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
}
