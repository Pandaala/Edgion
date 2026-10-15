use super::*;
use crate::conf_mgr::sync_runtime::ShutdownHandle;
use edgion_common::link_sys_core::providers::etcd::EtcdBootstrapConfig;
use edgion_common::link_sys_core::providers::etcd::EtcdLinkClient;

/// Build + connect an `EtcdLinkClient` against a local etcd, mirroring the
/// edgion-common ops.rs test helper.
async fn test_link_client() -> EtcdLinkClient {
    let config = EtcdBootstrapConfig {
        endpoints: vec!["http://localhost:2379".to_string()],
        auth: None,
        tls: None,
        timeout: None,
        keep_alive: None,
        namespace: None,
    };
    let client =
        EtcdLinkClient::from_bootstrap("default/etcd-le-test", config).expect("bootstrap construction must succeed");
    client.init().await.expect("etcd init must connect");
    client
}

/// A per-run unique election prefix so repeated runs never collide on a stale
/// leader key.
fn unique_prefix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("/edgion-test-le/{nanos}")
}

#[tokio::test]
#[ignore = "requires a real etcd serving v3election on localhost:2379"]
async fn single_candidate_becomes_leader_and_gate_opens() {
    let client = Arc::new(test_link_client().await);
    let cfg = EtcdLeaderElectionConfig {
        grpc_addr: "10.0.0.11:9090".into(),
        admin_addr: "10.0.0.11:8080".into(),
        lease_duration_secs: 6,
        renew_deadline_secs: 4,
        retry_period_secs: 1,
        identity: Some("ctrl-a".into()),
    };
    let elect = EtcdLeaderElection::new(client, &unique_prefix(), cfg).unwrap();
    let handle = elect.handle();
    let gate = elect.gate();
    let slot = Arc::new(ArcSwap::from_pointee(CancellationToken::new()));
    let sd = ShutdownHandle::new();
    let task = {
        let s = sd.signal();
        let sl = slot.clone();
        tokio::spawn(async move {
            elect.run(s, sl).await;
        })
    };

    // Within one lease it should win and the gate should open.
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if handle.is_leader() && gate.check().is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("should become leader within ~1 lease");

    assert_eq!(handle.current_leader().unwrap().admin_addr, "10.0.0.11:8080");

    sd.shutdown();
    let _ = task.await;
}

/// Poll `cond` every 100ms until it returns true or `dur` elapses. Returns
/// true if the condition held within the deadline.
async fn wait_until(dur: Duration, cond: impl Fn() -> bool) -> bool {
    tokio::time::timeout(dur, async {
        loop {
            if cond() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .is_ok()
}

/// A short test-tuned config with distinct advertised addresses/identity.
fn test_cfg(grpc: &str, admin: &str, identity: &str) -> EtcdLeaderElectionConfig {
    EtcdLeaderElectionConfig {
        grpc_addr: grpc.into(),
        admin_addr: admin.into(),
        lease_duration_secs: 6,
        renew_deadline_secs: 4,
        retry_period_secs: 1,
        identity: Some(identity.into()),
    }
}

/// Two candidates campaign on the SAME etcd + SAME election prefix. Exactly
/// one must win; the other stays a follower. When the leader shuts down, the
/// follower's blocked `campaign()` must unblock and take over. This is the
/// core active-passive HA guarantee (spec §4).
#[tokio::test]
#[ignore = "requires a real etcd serving v3election on localhost:2379"]
async fn two_candidates_one_leader() {
    let prefix = unique_prefix();

    let client_a = Arc::new(test_link_client().await);
    let client_b = Arc::new(test_link_client().await);
    let elect_a = EtcdLeaderElection::new(
        client_a,
        &prefix,
        test_cfg("10.0.0.11:9090", "10.0.0.11:8080", "ctrl-a"),
    )
    .unwrap();
    let elect_b = EtcdLeaderElection::new(
        client_b,
        &prefix,
        test_cfg("10.0.0.12:9090", "10.0.0.12:8080", "ctrl-b"),
    )
    .unwrap();

    let handle_a = elect_a.handle();
    let handle_b = elect_b.handle();
    let sd_a = ShutdownHandle::new();
    let sd_b = ShutdownHandle::new();
    let slot_a = Arc::new(ArcSwap::from_pointee(CancellationToken::new()));
    let slot_b = Arc::new(ArcSwap::from_pointee(CancellationToken::new()));

    let mut task_a = {
        let s = sd_a.signal();
        Some(tokio::spawn(async move { elect_a.run(s, slot_a).await }))
    };
    let mut task_b = {
        let s = sd_b.signal();
        Some(tokio::spawn(async move { elect_b.run(s, slot_b).await }))
    };

    // Within ~2 leases exactly one candidate must win.
    assert!(
        wait_until(Duration::from_secs(15), || handle_a.is_leader() || handle_b.is_leader()).await,
        "one of the two candidates must win leadership"
    );
    // Let the race settle, then assert mutual exclusion.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let a_leader = handle_a.is_leader();
    let b_leader = handle_b.is_leader();
    assert!(
        a_leader ^ b_leader,
        "exactly one leader expected (a={a_leader}, b={b_leader})"
    );

    // The FOLLOWER must learn the leader's identity while blocked in
    // campaign (the candidacy refresher, spec §4.3/§6.2): its Admin API 409
    // body depends on it.
    let (leader_admin, follower_handle) = if a_leader {
        ("10.0.0.11:8080", &handle_b)
    } else {
        ("10.0.0.12:8080", &handle_a)
    };
    assert!(
        wait_until(Duration::from_secs(5), || {
            follower_handle.current_leader().map(|l| l.admin_addr) == Some(leader_admin.to_string())
        })
        .await,
        "the follower must surface the live leader's admin_addr via its candidacy refresher"
    );

    // Shut the current leader down; the follower must take over.
    if a_leader {
        sd_a.shutdown();
        if let Some(t) = task_a.take() {
            let _ = t.await;
        }
        assert!(
            wait_until(Duration::from_secs(20), || handle_b.is_leader()).await,
            "follower B must take over after leader A resigns"
        );
    } else {
        sd_b.shutdown();
        if let Some(t) = task_b.take() {
            let _ = t.await;
        }
        assert!(
            wait_until(Duration::from_secs(20), || handle_a.is_leader()).await,
            "follower A must take over after leader B resigns"
        );
    }

    // Shut down whatever is still running and join, so no task leaks.
    sd_a.shutdown();
    sd_b.shutdown();
    if let Some(t) = task_a.take() {
        let _ = t.await;
    }
    if let Some(t) = task_b.take() {
        let _ = t.await;
    }
}

/// Regression for the create_revision-only guard bug (spec §4.4). A leader
/// holds a leader key at `{election_name}/{lease_hex}`. An external `put`
/// over that key WITHOUT a lease PRESERVES its create_revision but detaches
/// the lease and changes the value. A guard that only compared
/// create_revision would keep believing it is leader; the three-compare
/// guard txn must detect it and demote within ~renew_deadline.
#[tokio::test]
#[ignore = "requires a real etcd serving v3election on localhost:2379"]
async fn inplace_overwrite_demotes_leader() {
    let prefix = unique_prefix();
    let client = Arc::new(test_link_client().await);
    let elect = EtcdLeaderElection::new(
        client.clone(),
        &prefix,
        test_cfg("10.0.0.11:9090", "10.0.0.11:8080", "ctrl-a"),
    )
    .unwrap();
    let handle = elect.handle();
    let sd = ShutdownHandle::new();
    let slot = Arc::new(ArcSwap::from_pointee(CancellationToken::new()));
    let task = {
        let s = sd.signal();
        tokio::spawn(async move { elect.run(s, slot).await })
    };

    assert!(
        wait_until(Duration::from_secs(15), || handle.is_leader()).await,
        "candidate must first win leadership"
    );

    // Discover the actual v3election leader key. `get_prefix` returns keys
    // with the client namespace stripped; the test client has no namespace,
    // so the returned key is the full key and round-trips through
    // `put_string` (which re-applies the same — empty — namespace).
    let items = client
        .get_prefix(&format!("{prefix}/leader"))
        .await
        .expect("get_prefix must succeed");
    assert!(!items.is_empty(), "the leader key must exist under the election prefix");
    let leader_key = items[0].0.clone();

    // In-place overwrite WITHOUT a lease: create_revision preserved, lease
    // detached, value changed.
    client
        .put_string(&leader_key, "garbage-no-lease")
        .await
        .expect("external overwrite must succeed");

    // The guard txn (ticking every retry_period=1s) must catch the mismatch
    // and demote well within ~2*renew_deadline.
    assert!(
        wait_until(Duration::from_secs(15), || !handle.is_leader()).await,
        "leader must demote after its leader key is overwritten in place"
    );

    sd.shutdown();
    let _ = task.await;
}

/// Re-grant / re-campaign path (spec §4.5/§4.8). One election wins, then
/// shuts down cleanly — its resign + lease revoke releases the leader key.
/// A SECOND election on the same prefix must then win. This exercises the
/// full grant-lease → campaign path a second time against a prefix that
/// previously had a leader.
///
/// What it does NOT cover: injecting a dead session lease mid-flight (that
/// needs a fault proxy and lives in the integration suite); this test only
/// asserts the release-then-re-acquire sequence works end to end.
#[tokio::test]
#[ignore = "requires a real etcd serving v3election on localhost:2379"]
async fn candidate_survives_and_wins() {
    let prefix = unique_prefix();

    // First election wins, then shuts down cleanly (resign + lease revoke).
    let client1 = Arc::new(test_link_client().await);
    let elect1 =
        EtcdLeaderElection::new(client1, &prefix, test_cfg("10.0.0.11:9090", "10.0.0.11:8080", "ctrl-a")).unwrap();
    let handle1 = elect1.handle();
    let sd1 = ShutdownHandle::new();
    let slot1 = Arc::new(ArcSwap::from_pointee(CancellationToken::new()));
    let task1 = {
        let s = sd1.signal();
        tokio::spawn(async move { elect1.run(s, slot1).await })
    };
    assert!(
        wait_until(Duration::from_secs(15), || handle1.is_leader()).await,
        "first election must win"
    );
    sd1.shutdown();
    let _ = task1.await;

    // Second election on the same prefix must win once the key is released.
    let client2 = Arc::new(test_link_client().await);
    let elect2 =
        EtcdLeaderElection::new(client2, &prefix, test_cfg("10.0.0.12:9090", "10.0.0.12:8080", "ctrl-b")).unwrap();
    let handle2 = elect2.handle();
    let sd2 = ShutdownHandle::new();
    let slot2 = Arc::new(ArcSwap::from_pointee(CancellationToken::new()));
    let task2 = {
        let s = sd2.signal();
        tokio::spawn(async move { elect2.run(s, slot2).await })
    };
    assert!(
        wait_until(Duration::from_secs(15), || handle2.is_leader()).await,
        "second election must win after the first released the key"
    );
    // Winner identity is published locally before the leadership flag.
    assert_eq!(handle2.current_leader().unwrap().admin_addr, "10.0.0.12:8080");
    sd2.shutdown();
    let _ = task2.await;
}

/// Spec §4.8: a leaseless key under the election prefix (an out-of-band
/// `etcdctl put` on a NEW key, not an overwrite of the leader's) cannot
/// preempt a live leader, but once it becomes the lowest create_revision it
/// would strand the election — the sweeper must delete it. The live leader's
/// own (lease-attached) key must survive the sweep.
#[tokio::test]
#[ignore = "requires a real etcd serving v3election on localhost:2379"]
async fn sweeper_removes_leaseless_key_but_not_leader_key() {
    let prefix = unique_prefix();
    let client = Arc::new(test_link_client().await);
    let elect = EtcdLeaderElection::new(
        client.clone(),
        &prefix,
        test_cfg("10.0.0.11:9090", "10.0.0.11:8080", "ctrl-a"),
    )
    .unwrap();
    let handle = elect.handle();
    let sd = ShutdownHandle::new();
    let slot = Arc::new(ArcSwap::from_pointee(CancellationToken::new()));
    let task = {
        let s = sd.signal();
        tokio::spawn(async move { elect.run(s, slot).await })
    };
    assert!(
        wait_until(Duration::from_secs(15), || handle.is_leader()).await,
        "candidate must win leadership first"
    );

    // Plant a leaseless key under the election prefix, out of band.
    let stray = format!("{}/leader/stray-manual-key", prefix.trim_end_matches('/'));
    client
        .put_string(&stray, "garbage")
        .await
        .expect("out-of-band put must succeed");

    // One sweep period is lease_duration (6s here); allow two plus slack.
    // (`wait_until` takes a sync closure, so poll the async probe directly.)
    let mut swept = false;
    for _ in 0..30 {
        if matches!(client.get_kv(&stray).await, Ok(None)) {
            swept = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        swept,
        "the sweeper must delete the leaseless stray key within ~2 sweep periods"
    );

    // The leader must still be leader and its own key intact.
    assert!(
        handle.is_leader(),
        "sweeping the stray must not disturb the live leader"
    );

    sd.shutdown();
    let _ = task.await;
}

// NOTE: `transient_guard_error_does_not_demote` (spec §4.5.1 — a transient
// guard-txn transport error `Ok(Err(_))` must NOT demote, only an
// authoritative `Ok(Ok(false))` may) is intentionally NOT implemented here.
// The classification lives in `spawn_guard_task`: `Ok(Err(_)) | Err(_)` is a
// no-op ("transport blip; do NOT demote") while `Ok(Ok(false))` returns
// `TaskExit::GuardLost`. Reliably injecting a *transient* etcd transport
// error against a live etcd (without also taking down the authoritative
// signal) requires a fault-injection proxy, which is impractical in this
// unit-level harness. That path is exercised by the integration suite's
// network-partition / fault-injection scenarios (Plan A Task 14 / the manual
// partition test), not here. A contrived unit shim would only re-assert the
// literal match arms above without adding real confidence, so it is omitted
// deliberately rather than faked.

/// Construct only; none of these lifecycle tests initializes a network client.
fn local_election() -> EtcdLeaderElection {
    let client = EtcdLinkClient::from_bootstrap(
        "default/local-election",
        EtcdBootstrapConfig {
            endpoints: vec!["http://localhost:2379".into()],
            auth: None,
            tls: None,
            timeout: None,
            keep_alive: None,
            namespace: None,
        },
    )
    .unwrap();
    EtcdLeaderElection::new(
        Arc::new(client),
        "/local-election",
        test_cfg("10.0.0.11:9090", "10.0.0.11:8080", "cold-identity"),
    )
    .unwrap()
}

fn token_slot() -> Arc<ArcSwap<CancellationToken>> {
    Arc::new(ArcSwap::from_pointee(CancellationToken::new()))
}

#[tokio::test]
async fn winner_publishes_cold_campaign_identity_and_fresh_token() {
    let mut election = local_election();
    let cold: LeaderInfo = serde_json::from_slice(&election.value_bytes).unwrap();
    assert_eq!(cold, election.local_info);
    election.config.identity = Some("changed-after-construction".into());
    election.config.grpc_addr = "10.0.0.99:9090".into();
    election.config.admin_addr = "10.0.0.99:8080".into();
    let slot = token_slot();
    let old = slot.load_full();
    old.cancel();
    let token = CancellationToken::new();
    let mut signal = election.handle().subscribe();
    election.publish_term(&slot, &token);
    signal.changed().await.unwrap();
    assert!(*signal.borrow());
    assert!(!slot.load().is_cancelled());
    assert_eq!(election.handle().current_leader(), Some(cold));
    assert!(election.gate().check().is_ok());
    token.cancel();
    assert!(slot.load().is_cancelled(), "slot must contain this term's token");
}

#[tokio::test]
async fn candidate_diagnostic_is_dropped_before_winner_publication() {
    use tokio::sync::oneshot;

    for late_ready_at_win in [false, true] {
        let election = local_election();
        let (started_tx, started_rx) = oneshot::channel();
        let (late_tx, late_rx) = oneshot::channel();
        let mut late_tx = Some(late_tx);
        let follower = LeaderInfo {
            identity: "previous-leader".into(),
            grpc_addr: "10.0.0.12:9090".into(),
            admin_addr: "10.0.0.12:8080".into(),
        };
        let diagnostic = async {
            election.state.set_current_leader(Some(follower.clone()));
            started_tx.send(()).unwrap();
            late_rx.await.unwrap();
            election.state.set_current_leader(Some(follower));
            std::future::pending::<Infallible>().await
        };
        let campaign = async {
            started_rx.await.unwrap();
            assert_eq!(election.handle().current_leader().unwrap().identity, "previous-leader");
            if late_ready_at_win {
                late_tx.take().unwrap().send(()).unwrap();
            }
            CampaignResult::Won(LeaderKey::default())
        };
        assert!(matches!(
            campaign_with_refresher(campaign, diagnostic).await,
            CampaignResult::Won(_)
        ));
        election.publish_term(&token_slot(), &CancellationToken::new());
        if let Some(tx) = late_tx {
            assert!(tx.send(()).is_err(), "pending diagnostic receiver must be dropped");
        }
        tokio::task::yield_now().await;
        assert_eq!(election.handle().current_leader(), Some(election.local_info.clone()));
    }
}

#[tokio::test]
async fn candidate_exit_drops_pending_diagnostic_without_publishing() {
    use tokio::sync::oneshot;

    for shutdown in [false, true] {
        let election = local_election();
        let (started_tx, started_rx) = oneshot::channel();
        let (late_tx, late_rx) = oneshot::channel::<()>();
        let diagnostic = async {
            started_tx.send(()).unwrap();
            late_rx.await.unwrap();
            std::future::pending::<Infallible>().await
        };
        let result = campaign_with_refresher(
            async {
                started_rx.await.unwrap();
                if shutdown {
                    CampaignResult::Shutdown
                } else {
                    CampaignResult::LeaseDead
                }
            },
            diagnostic,
        )
        .await;
        assert!(matches!(
            (shutdown, result),
            (true, CampaignResult::Shutdown) | (false, CampaignResult::LeaseDead)
        ));
        assert!(late_tx.send(()).is_err());
        assert!(!election.handle().is_leader());
        assert!(election.handle().current_leader().is_none());
    }
}

#[tokio::test]
async fn diagnostic_panic_preserves_pending_campaign_and_shutdown() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::oneshot;

    for shutdown in [false, true] {
        let starts = AtomicUsize::new(0);
        let sd = ShutdownHandle::new();
        let mut signal = sd.signal();
        let (panic_tx, panic_rx) = oneshot::channel::<()>();
        let (win_tx, win_rx) = oneshot::channel::<()>();
        let campaign = async {
            starts.fetch_add(1, Ordering::SeqCst);
            tokio::select! {
                _ = signal.wait() => CampaignResult::Shutdown,
                _ = win_rx => CampaignResult::Won(LeaderKey::default()),
            }
        };
        let diagnostic = async {
            let _notify_on_drop = panic_tx;
            panic!("injected candidate diagnostic panic");
            #[allow(unreachable_code)]
            std::future::pending::<Infallible>().await
        };
        let controller = async {
            assert!(panic_rx.await.is_err(), "diagnostic panic must drop its owned state");
            if shutdown {
                sd.shutdown();
            } else {
                win_tx.send(()).unwrap();
            }
        };
        let (result, ()) = tokio::join!(campaign_with_refresher(campaign, diagnostic), controller);
        assert!(matches!(
            (shutdown, result),
            (true, CampaignResult::Shutdown) | (false, CampaignResult::Won(_))
        ));
        assert_eq!(starts.load(Ordering::SeqCst), 1, "campaign must not restart");
    }
}

#[tokio::test]
async fn supervised_term_demotes_and_cancels_before_cleanup() {
    use std::time::Instant;

    for cause in ["shutdown", "lease-dead", "guard-lost", "task-panic", "renew-expired"] {
        let election = local_election();
        let slot = token_slot();
        let token = CancellationToken::new();
        election.publish_term(&slot, &token);
        let sd = ShutdownHandle::new();
        let mut tasks = JoinSet::new();
        match cause {
            "shutdown" => sd.shutdown(),
            "lease-dead" => {
                tasks.spawn(async { TaskExit::LeaseDead });
            }
            "guard-lost" => {
                tasks.spawn(async { TaskExit::GuardLost });
            }
            "task-panic" => {
                tasks.spawn(async {
                    panic!("injected term task panic");
                });
            }
            "renew-expired" => election
                .state
                .record_keepalive_at(Instant::now(), wall_now().saturating_sub(5_000)),
            _ => unreachable!(),
        }
        let outcome = timeout(
            Duration::from_secs(1),
            election.supervise_term(&mut sd.signal(), &mut tasks, &token),
        )
        .await
        .expect("local supervision must not wait on network cleanup");
        match outcome {
            SuperviseOutcome::Shutdown => assert_eq!(cause, "shutdown"),
            SuperviseOutcome::Demoted => assert_ne!(cause, "shutdown"),
        }
        assert!(!election.handle().is_leader(), "{cause}");
        assert!(election.gate().check().is_err(), "{cause}");
        assert!(election.handle().current_leader().is_none(), "{cause}");
        assert!(slot.load().is_cancelled(), "{cause}");
        assert!(election.state.elapsed_since_last_ok() > Duration::from_secs(60));

        let successor = CancellationToken::new();
        election.publish_term(&slot, &successor);
        token.cancel();
        assert!(
            !slot.load().is_cancelled(),
            "old term cancellation must not cancel successor"
        );
        assert!(election.gate().check().is_ok());
    }
}
