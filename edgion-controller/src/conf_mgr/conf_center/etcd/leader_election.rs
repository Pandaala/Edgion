//! Etcd-mode leader election (spec §4). Two-task-per-term structure: a keepalive
//! DRIVER + a guard-txn task do all etcd I/O and publish into `LeaderState`; a
//! term SUPERVISOR awaits ONLY timers and `JoinSet` completions so it can never
//! be starved by a hung etcd call (spec §4.1/§4.2).

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use arc_swap::ArcSwap;
use tokio::task::JoinSet;
use tokio::time::{interval, timeout, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use super::leadership_gate::{wall_now, LeaderHandle, LeaderInfo, LeaderState, LeadershipGate};
use crate::conf_mgr::sync_runtime::metrics::controller_metrics;
use crate::conf_mgr::sync_runtime::ShutdownSignal;
use crate::config::conf_center::etcd::EtcdLeaderElectionConfig;
use edgion_common::link_sys_core::providers::etcd::{EtcdLinkClient, LeaderKey};

/// Owns the shared `LeaderState` plus the config/client needed to campaign for
/// and hold a leadership term. `run` is the never-returning outer loop; `handle`
/// and `gate` expose read-only views for the lifecycle gate, the Admin API 409
/// gate, and the pre-write fence.
#[allow(dead_code)]
pub struct EtcdLeaderElection {
    client: Arc<EtcdLinkClient>,
    state: Arc<LeaderState>,
    config: EtcdLeaderElectionConfig,
    /// Election name: "{key_prefix}/leader".
    election_name: String,
    /// Serialized `LeaderInfo` written as the leader-key value.
    value_bytes: Vec<u8>,
}

#[allow(dead_code)]
impl EtcdLeaderElection {
    pub fn new(client: Arc<EtcdLinkClient>, key_prefix: &str, config: EtcdLeaderElectionConfig) -> Result<Self> {
        config.validate()?;
        let info = LeaderInfo {
            identity: config.resolved_identity(),
            grpc_addr: config.grpc_addr.clone(),
            admin_addr: config.admin_addr.clone(),
        };
        let value_bytes = serde_json::to_vec(&info)?;
        let state = LeaderState::new(Duration::from_secs(config.lease_duration_secs));
        Ok(Self {
            client,
            state,
            config,
            election_name: format!("{}/leader", key_prefix.trim_end_matches('/')),
            value_bytes,
        })
    }

    pub fn handle(&self) -> LeaderHandle {
        LeaderHandle::from_state(self.state.clone())
    }

    pub fn gate(&self) -> LeadershipGate {
        LeadershipGate::from_state(self.state.clone())
    }

    /// Outer election loop. Never returns until shutdown.
    ///
    /// `shutdown` is taken by value (`ShutdownSignal::wait` needs `&mut self`);
    /// the outer loop and its helpers borrow it mutably in sequence.
    pub async fn run(&self, mut shutdown: ShutdownSignal, term_token_slot: Arc<ArcSwap<CancellationToken>>) {
        // Probe v3election once, warn loudly if absent (spec §3.2).
        if let Ok(mut sess) = self.client.election_session().await {
            if let Err(e) = sess.probe().await {
                tracing::error!(
                    component = "etcd_election",
                    error = %e,
                    "v3election unavailable; leader election cannot run"
                );
            }
        }
        let mut demotions: u32 = 0;
        loop {
            if shutdown.is_shutdown() {
                return;
            }
            match self.one_term(&mut shutdown, &term_token_slot).await {
                TermOutcome::Shutdown => return,
                TermOutcome::Demoted => {
                    demotions = demotions.saturating_add(1);
                    // Spec §4.5.2: back off before re-campaigning.
                    let backoff = backoff_with_jitter(demotions);
                    tracing::warn!(
                        component = "etcd_election",
                        demotions,
                        backoff_ms = backoff.as_millis() as u64,
                        "demoted; backing off before re-campaign"
                    );
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        _ = shutdown.wait() => return,
                    }
                }
                TermOutcome::WonAndSurvived => {
                    // Reset the stability counter after a clean term.
                    demotions = 0;
                }
            }
        }
    }

    async fn one_term(
        &self,
        shutdown: &mut ShutdownSignal,
        term_token_slot: &Arc<ArcSwap<CancellationToken>>,
    ) -> TermOutcome {
        // 1. Session lease. On failure, retry until shutdown.
        let lease = match self.grant_session_lease(shutdown).await {
            Some(l) => l,
            None => return TermOutcome::Shutdown,
        };

        // 2. Candidacy: campaign() blocks until won; cancel on shutdown.
        let lk = match self.campaign(lease, shutdown).await {
            CampaignResult::Won(lk) => lk,
            CampaignResult::LeaseDead => {
                // Re-grant a fresh lease next loop.
                let _ = timeout(Duration::from_secs(2), self.client.lease_revoke(lease)).await;
                return TermOutcome::Demoted;
            }
            CampaignResult::Shutdown => {
                let _ = timeout(Duration::from_secs(2), self.client.lease_revoke(lease)).await;
                return TermOutcome::Shutdown;
            }
        };

        // 3. WON. Publish a fresh term token BEFORE anyone can serve as leader
        //    (spec §6.1), then flip the local flag.
        let term_token = CancellationToken::new();
        term_token_slot.store(Arc::new(term_token.clone()));
        self.state.record_keepalive_now();
        self.state.set_leader(true);
        self.refresh_current_leader().await;
        controller_metrics().set_leader(true);
        tracing::info!(
            component = "etcd_election",
            identity = %self.config.resolved_identity(),
            "won leadership"
        );

        // 4. Term tasks: keepalive driver + guard-txn task under one JoinSet/token.
        let mut tasks: JoinSet<TaskExit> = JoinSet::new();
        self.spawn_keepalive_driver(&mut tasks, lease, term_token.clone());
        self.spawn_guard_task(&mut tasks, lk.clone(), lease, term_token.clone());

        // 5. Term supervisor: ONLY timers + joins (never an etcd call).
        let outcome = self.supervise_term(shutdown, &mut tasks).await;

        // 6. Demotion (spec §4.5). Flag FIRST (local, cannot block), THEN
        //    cancel + abort + bounded join, THEN best-effort resign/revoke LAST.
        self.state.set_leader(false);
        self.state.mark_lease_invalid();
        controller_metrics().set_leader(false);
        term_token.cancel();
        tasks.abort_all();
        let _ = timeout(Duration::from_secs(2), async {
            while tasks.join_next().await.is_some() {}
        })
        .await;
        // Best-effort resign + lease revoke, each bounded.
        if let Ok(mut sess) = self.client.election_session().await {
            let _ = timeout(Duration::from_secs(2), sess.resign(lk)).await;
        }
        let _ = timeout(Duration::from_secs(2), self.client.lease_revoke(lease)).await;

        match outcome {
            SuperviseOutcome::Shutdown => TermOutcome::Shutdown,
            SuperviseOutcome::Demoted => TermOutcome::Demoted,
        }
    }

    // ---- helpers ----

    async fn grant_session_lease(&self, shutdown: &mut ShutdownSignal) -> Option<i64> {
        loop {
            tokio::select! {
                _ = shutdown.wait() => return None,
                r = self.client.lease_grant(self.config.lease_duration_secs as i64) => {
                    match r {
                        Ok(l) => return Some(l),
                        Err(e) => {
                            tracing::warn!(
                                component = "etcd_election",
                                error = %e,
                                "lease_grant failed; retrying"
                            );
                            tokio::time::sleep(Duration::from_secs(self.config.retry_period_secs)).await;
                        }
                    }
                }
            }
        }
    }

    async fn campaign(&self, lease: i64, shutdown: &mut ShutdownSignal) -> CampaignResult {
        let mut sess = match self.client.election_session().await {
            Ok(s) => s,
            // Treat a session-build failure as retryable.
            Err(_) => return CampaignResult::LeaseDead,
        };
        tokio::select! {
            _ = shutdown.wait() => CampaignResult::Shutdown,
            r = sess.campaign(&self.election_name, self.value_bytes.clone(), lease) => match r {
                Ok(lk) => CampaignResult::Won(lk),
                // Includes lease-not-found; re-grant next loop.
                Err(_) => CampaignResult::LeaseDead,
            }
        }
    }

    fn spawn_keepalive_driver(&self, tasks: &mut JoinSet<TaskExit>, lease: i64, token: CancellationToken) {
        let client = self.client.clone();
        let state = self.state.clone();
        let retry = Duration::from_secs(self.config.retry_period_secs);
        tasks.spawn(async move {
            loop {
                if token.is_cancelled() {
                    return TaskExit::Cancelled;
                }
                // Establish. Err here = authoritative lease death (spec §4.2 fact 4).
                let est = tokio::select! {
                    _ = token.cancelled() => return TaskExit::Cancelled,
                    r = timeout(retry * 2, client.lease_keep_alive(lease)) => r,
                };
                let (mut keeper, mut stream) = match est {
                    Ok(Ok(pair)) => pair,
                    Ok(Err(_)) => return TaskExit::LeaseDead, // establish saw ttl<=0
                    Err(_) => continue,                       // establish timed out — retry
                };
                // A successful establish means etcd-client read a first response with
                // ttl > 0 inside the call (ttl <= 0 would have returned Err above), so
                // it IS an authoritative keepalive success — record it. Without this,
                // a connection that lets establish succeed but then immediately ends
                // the stream (before the first tick's response) would re-establish in a
                // loop, each time confirming the lease is alive, yet never refresh
                // last_success, and the supervisor would demote on a live lease.
                state.record_keepalive_now();
                let mut tick = interval(retry);
                tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
                loop {
                    tokio::select! {
                        _ = token.cancelled() => return TaskExit::Cancelled,
                        _ = tick.tick() => {
                            // keep_alive() only pushes into a bounded channel; success proves nothing.
                            if keeper.keep_alive().await.is_err() {
                                break; // re-establish
                            }
                        }
                        msg = stream.message() => match msg {
                            Ok(Some(resp)) if resp.ttl() > 0 => state.record_keepalive_now(),
                            Ok(Some(_)) => return TaskExit::LeaseDead, // ttl == 0
                            Ok(None) | Err(_) => break,                // stream ended/errored — re-establish
                        }
                    }
                }
            }
        });
    }

    fn spawn_guard_task(&self, tasks: &mut JoinSet<TaskExit>, lk: LeaderKey, lease: i64, token: CancellationToken) {
        let client = self.client.clone();
        let key = String::from_utf8_lossy(lk.key()).to_string();
        // LeaderKey::rev() is the CREATE revision of our leader key.
        let create_rev = lk.rev();
        let value = self.value_bytes.clone();
        let retry = Duration::from_secs(self.config.retry_period_secs);
        tasks.spawn(async move {
            let mut tick = interval(retry);
            tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = token.cancelled() => return TaskExit::Cancelled,
                    _ = tick.tick() => {
                        match timeout(retry, client.leader_guard_txn(&key, create_rev, lease, &value)).await {
                            Ok(Ok(true)) => { /* still ours */ }
                            Ok(Ok(false)) => return TaskExit::GuardLost, // authoritative: not ours
                            Ok(Err(_)) | Err(_) => { /* transport blip; do NOT demote */ }
                        }
                    }
                }
            }
        });
    }

    /// Term supervisor (spec §4.1). Awaits ONLY `shutdown.wait()`, a timer tick,
    /// and `tasks.join_next()` — NEVER an etcd call.
    async fn supervise_term(&self, shutdown: &mut ShutdownSignal, tasks: &mut JoinSet<TaskExit>) -> SuperviseOutcome {
        let renew = Duration::from_secs(self.config.renew_deadline_secs);
        let mut tick = interval(Duration::from_secs(self.config.retry_period_secs));
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // Pause detection (spec §4.6) MUST measure in WALL time: CLOCK_MONOTONIC
        // (Rust `Instant`) freezes across a VM suspend, so a monotonic tick-gap can
        // never observe the suspend it exists to catch. This is a defense-in-depth
        // signal ON TOP of cond 1: cond 1 fires only once the wall gap since the last
        // keepalive success exceeds `renew_deadline`, whereas this fires on a suspend
        // shorter than `renew_deadline` but still grossly longer than `retry_period`,
        // demoting conservatively before the pre-write gate is ever consulted.
        let pause_threshold_ms = self.config.retry_period_secs.saturating_mul(3).saturating_mul(1000);
        let mut last_tick_wall = wall_now();
        loop {
            tokio::select! {
                _ = shutdown.wait() => return SuperviseOutcome::Shutdown,
                _ = tick.tick() => {
                    // Cond 1: renew deadline (un-starvable — no etcd call here).
                    if !self.state_lease_fresh_within(renew) {
                        return SuperviseOutcome::Demoted;
                    }
                    // Pause detection: a wall tick-gap >> retry_period ⇒ suspect a
                    // suspend / stalled worker ⇒ demote.
                    let now_wall = wall_now();
                    if now_wall.saturating_sub(last_tick_wall) > pause_threshold_ms {
                        return SuperviseOutcome::Demoted;
                    }
                    last_tick_wall = now_wall;
                }
                // Cond 4: any term task exiting non-cancel ⇒ demote. Guard the arm
                // so an empty JoinSet does not busy-spin (spec §4.5.1).
                Some(res) = tasks.join_next(), if !tasks.is_empty() => {
                    match res {
                        Ok(TaskExit::Cancelled) => { /* expected during demotion */ }
                        Ok(TaskExit::LeaseDead) | Ok(TaskExit::GuardLost) => {
                            return SuperviseOutcome::Demoted;
                        }
                        Err(_) => return SuperviseOutcome::Demoted, // panic
                    }
                }
            }
        }
    }

    fn state_lease_fresh_within(&self, within: Duration) -> bool {
        self.state.elapsed_since_last_ok() < within
    }

    async fn refresh_current_leader(&self) {
        if let Ok(mut sess) = self.client.election_session().await {
            if let Ok(Some(bytes)) = sess.leader(&self.election_name).await {
                if let Ok(info) = serde_json::from_slice::<LeaderInfo>(&bytes) {
                    self.state.set_current_leader(Some(info));
                }
            }
        }
    }
}

fn backoff_with_jitter(attempt: u32) -> Duration {
    use rand::Rng;
    let base = 200u64.saturating_mul(1u64 << attempt.min(6)); // cap ~12.8s
    let capped = base.min(15_000);
    let jitter = rand::rng().random_range(0..=capped / 2);
    Duration::from_millis(capped / 2 + jitter)
}

#[allow(dead_code)]
enum TermOutcome {
    Shutdown,
    Demoted,
    WonAndSurvived,
}

enum CampaignResult {
    Won(LeaderKey),
    LeaseDead,
    Shutdown,
}

enum SuperviseOutcome {
    Shutdown,
    Demoted,
}

#[derive(Debug)]
enum TaskExit {
    Cancelled,
    LeaseDead,
    GuardLost,
}

#[cfg(test)]
mod tests {
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
            auto_sync_interval: None,
            max_call_send_size: None,
            max_call_recv_size: None,
            user_agent: None,
            reject_old_cluster: None,
            observability: None,
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
}
