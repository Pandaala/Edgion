use super::*;
use crate::conf_mgr::sync_runtime::ShutdownHandle;
use edgion_common::link_sys_core::credential::CredentialState;
use edgion_common::link_sys_core::providers::etcd::EtcdLinkClient;
use edgion_resources::resources::link_sys::etcd::EtcdClientConfig;

/// Build + connect an `EtcdLinkClient` against a local etcd, mirroring the
/// edgion-common ops.rs test helper.
async fn test_link_client() -> EtcdLinkClient {
    let config = EtcdClientConfig {
        endpoints: vec!["http://localhost:2379".to_string()],
        auth: None,
        tls: None,
        timeout: None,
        keep_alive: None,
        namespace: None,
    };
    let client = EtcdLinkClient::from_config("default/etcd-le-test", &config, &CredentialState::NotRequired)
        .expect("from_config must succeed");
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
    // `current_leader` is refreshed one etcd round-trip AFTER `is_leader`
    // flips, so wait for it rather than reading it immediately.
    assert!(
        wait_until(Duration::from_secs(5), || handle2.current_leader().is_some()).await,
        "current_leader must be populated shortly after winning"
    );
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
