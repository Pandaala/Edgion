//! Etcd-mode leader election (spec §4). Two-task-per-term structure: a keepalive
//! DRIVER + a guard-txn task do all etcd I/O and publish into `LeaderState`; a
//! term SUPERVISOR awaits ONLY timers and `JoinSet` completions so it can never
//! be starved by a hung etcd call (spec §4.1/§4.2).

use std::convert::Infallible;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use arc_swap::ArcSwap;
use futures::FutureExt;
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
    /// The exact cold identity serialized for every campaign.
    local_info: LeaderInfo,
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
            local_info: info,
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
        // Term-independent leaseless-key sweeper (spec §4.8): a key under the
        // election prefix with no lease (an out-of-band `etcdctl put`) can never
        // preempt a live leader, but once it becomes the lowest create_revision
        // it would strand the election forever. Sweep it periodically.
        let sweeper = self.spawn_leaseless_sweeper();
        let mut demotions: u32 = 0;
        loop {
            if shutdown.is_shutdown() {
                break;
            }
            match self.one_term(&mut shutdown, &term_token_slot).await {
                TermOutcome::Shutdown => break,
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
                        _ = shutdown.wait() => break,
                    }
                }
                TermOutcome::WonAndSurvived => {
                    // Reset the stability counter after a clean term.
                    demotions = 0;
                }
            }
        }
        sweeper.abort();
    }

    async fn one_term(
        &self,
        shutdown: &mut ShutdownSignal,
        term_token_slot: &Arc<ArcSwap<CancellationToken>>,
    ) -> TermOutcome {
        // 1. Session lease. On failure, retry until shutdown.
        let Some(lease) = self.grant_session_lease(shutdown).await else {
            return TermOutcome::Shutdown;
        };

        // 2. Keepalive driver — started BEFORE campaign, because the lease must
        //    be kept alive continuously IN BOTH ROLES (spec §4.2). campaign()
        //    can block for arbitrarily long (minutes) while a leader is healthy;
        //    with the driver started only at win, the 6-15s session lease would
        //    expire mid-candidacy and the eventual win would carry a dead lease:
        //    the first post-win establish returns LeaseKeepAliveError and the
        //    brand-new leader demotes within milliseconds (observed in the e2e
        //    failover test as a won->demoted->re-won blip 30ms apart).
        //    `term_token` is created here so the driver has a cancel scope, but
        //    it is PUBLISHED into the slot only at win (§6.1 ordering).
        let term_token = CancellationToken::new();
        let mut tasks: JoinSet<TaskExit> = JoinSet::new();
        self.spawn_keepalive_driver(&mut tasks, lease, term_token.clone());

        // 3. Candidacy: campaign() blocks until won; cancel on shutdown or on
        //    candidate lease death (the driver exiting LeaseDead — spec §4.2:
        //    "a candidate silently re-grants"). While blocked, a candidacy-scoped
        //    refresher keeps `current_leader` fresh so the Admin API 409 body can
        //    name the actual leader (spec §4.3/§6.2).
        let campaign_result = campaign_with_refresher(
            self.campaign(lease, shutdown, &mut tasks),
            self.refresh_candidate_leader(),
        )
        .await;
        let lk = match campaign_result {
            CampaignResult::Won(lk) => lk,
            CampaignResult::LeaseDead => {
                // Tear down the candidacy tasks and re-grant a fresh lease next loop.
                term_token.cancel();
                tasks.abort_all();
                let _ = timeout(Duration::from_secs(2), async {
                    while tasks.join_next().await.is_some() {}
                })
                .await;
                let _ = timeout(Duration::from_secs(2), self.client.lease_revoke(lease)).await;
                return TermOutcome::Demoted;
            }
            CampaignResult::Shutdown => {
                term_token.cancel();
                tasks.abort_all();
                let _ = timeout(Duration::from_secs(2), async {
                    while tasks.join_next().await.is_some() {}
                })
                .await;
                let _ = timeout(Duration::from_secs(2), self.client.lease_revoke(lease)).await;
                return TermOutcome::Shutdown;
            }
        };

        // 4. WON. Publish the term token BEFORE anyone can serve as leader
        //    (spec §6.1), then flip the local flag. The keepalive driver has
        //    been running since step 2 and simply continues into the term.
        self.publish_term(term_token_slot, &term_token);

        // 4b. Remaining term task: the guard txn joins the same JoinSet/token.
        self.spawn_guard_task(&mut tasks, lk.clone(), lease, term_token.clone());

        // 5. Term supervisor: ONLY timers + joins (never an etcd call).
        let outcome = self.supervise_term(shutdown, &mut tasks, &term_token).await;

        // 6. Demotion (spec §4.5). Flag FIRST (local, cannot block), THEN
        //    cancel + abort + bounded join, THEN best-effort resign/revoke LAST.
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

    /// Publish only local state; no diagnostic I/O can delay term supervision.
    fn publish_term(&self, term_token_slot: &Arc<ArcSwap<CancellationToken>>, term_token: &CancellationToken) {
        term_token_slot.store(Arc::new(term_token.clone()));
        self.state.record_keepalive_now();
        self.state.set_current_leader(Some(self.local_info.clone()));
        self.state.set_leader(true);
        controller_metrics().set_leader(true);
        controller_metrics().inc_leader_transitions();
        tracing::info!(
            component = "etcd_election",
            identity = %self.local_info.identity,
            "won leadership"
        );
    }

    /// Resolve local authority before returning to any asynchronous cleanup.
    async fn supervise_term(
        &self,
        shutdown: &mut ShutdownSignal,
        tasks: &mut JoinSet<TaskExit>,
        term_token: &CancellationToken,
    ) -> SuperviseOutcome {
        let outcome = self.wait_for_term_end(shutdown, tasks).await;
        self.state.set_leader(false);
        self.state.mark_lease_invalid();
        // Our own cached identity is now stale; the candidacy refresher of the
        // next loop iteration repopulates it with the successor.
        self.state.set_current_leader(None);
        controller_metrics().set_leader(false);
        controller_metrics().inc_leader_transitions();
        term_token.cancel();
        outcome
    }

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

    /// Campaign for leadership. `tasks` holds the already-running keepalive
    /// driver: if it exits during candidacy (lease death, a panic, or any other
    /// non-cancel exit) the blocked `campaign()` is abandoned — its lease-keyed
    /// key is worthless once the lease dies — and the caller re-grants
    /// (spec §4.2 "a candidate silently re-grants").
    async fn campaign(
        &self,
        lease: i64,
        shutdown: &mut ShutdownSignal,
        tasks: &mut JoinSet<TaskExit>,
    ) -> CampaignResult {
        let Ok(mut sess) = self.client.election_session().await else {
            // Treat a session-build failure as retryable.
            return CampaignResult::LeaseDead;
        };
        tokio::select! {
            _ = shutdown.wait() => CampaignResult::Shutdown,
            // Guard the arm: join_next() on an empty JoinSet is Ready(None)
            // immediately and would busy-spin the select (spec §4.5.1 cond 4).
            Some(exit) = tasks.join_next(), if !tasks.is_empty() => match exit {
                // The driver only exits Cancelled when the term token fires,
                // which never happens during candidacy — treat it like death.
                Ok(TaskExit::Cancelled) | Ok(TaskExit::LeaseDead) | Ok(TaskExit::GuardLost) | Err(_) => {
                    CampaignResult::LeaseDead
                }
            },
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
    async fn wait_for_term_end(
        &self,
        shutdown: &mut ShutdownSignal,
        tasks: &mut JoinSet<TaskExit>,
    ) -> SuperviseOutcome {
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

    /// Leaseless-key sweeper (spec §4.8). Every `lease_duration`, scan the
    /// election prefix and delete keys carrying no lease. `v3election` keys are
    /// always lease-attached (`{name}/{lease_id_hex}`), so `lease == 0` can only
    /// be an out-of-band write. Discipline: an `Err` from any probe means do NOT
    /// delete; the delete itself is guarded on the observed `mod_revision` so a
    /// concurrent legitimate write can never be clobbered.
    fn spawn_leaseless_sweeper(&self) -> tokio::task::JoinHandle<()> {
        let client = self.client.clone();
        let prefix = self.election_name.clone();
        let period = Duration::from_secs(self.config.lease_duration_secs);
        let bound = Duration::from_secs(self.config.retry_period_secs);
        tokio::spawn(async move {
            let mut tick = interval(period);
            tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                // Transport trouble anywhere: skip and sweep next period.
                let Ok(Ok(keys)) = timeout(bound, client.get_prefix(&prefix)).await else {
                    continue;
                };
                for (key, _value, _rev) in keys {
                    // Err or already gone: do not delete.
                    let Ok(Ok(Some(kv))) = timeout(bound, client.get_kv(&key)).await else {
                        continue;
                    };
                    if kv.lease != 0 {
                        continue;
                    }
                    if let Ok(Ok(true)) = timeout(bound, client.txn_delete_if_mod_revision(&key, kv.mod_revision)).await
                    {
                        tracing::warn!(
                            component = "etcd_election",
                            key = %key,
                            "swept leaseless key under the election prefix (out-of-band write)"
                        );
                    }
                }
            }
        })
    }

    /// Candidacy-scoped `current_leader` refresher (spec §4.3): while `campaign()`
    /// blocks, poll `leader()` on the `retry_period` cadence so a follower's Admin
    /// API 409 body names the live leader. Dropped before campaign ownership returns;
    /// every etcd call is explicitly time-bounded (spec §4.1).
    ///
    /// The `ElectionSession` is REUSED across ticks — `election_session()` dials a
    /// brand-new etcd connection, and per-tick dialing would be steady-state
    /// connection churn on every standby forever (review finding). It is rebuilt
    /// whenever a tick yields no leader: `leader()` deliberately collapses
    /// transport failure and genuine vacancy into `None` (etcd reports vacancy as
    /// an error too, see `ElectionSession::leader`), so "no leader" is also the
    /// only reliable dead-connection signal. Steady state (leader known) does
    /// zero dials; a vacancy/outage window degrades to one dial per tick, which
    /// is bounded by the window itself.
    async fn refresh_candidate_leader(&self) -> Infallible {
        let client = self.client.clone();
        let state = self.state.clone();
        let name = self.election_name.clone();
        let retry = Duration::from_secs(self.config.retry_period_secs);
        let mut tick = interval(retry);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut session: Option<edgion_common::link_sys_core::providers::etcd::ElectionSession> = None;
        loop {
            tick.tick().await;
            if session.is_none() {
                session = match timeout(retry, client.election_session()).await {
                    Ok(Ok(s)) => Some(s),
                    _ => None, // dial failed/timed out: retry next tick
                };
            }
            let Some(sess) = session.as_mut() else { continue };
            match timeout(retry, sess.leader(&name)).await {
                Ok(Ok(Some(bytes))) => {
                    if let Ok(info) = serde_json::from_slice::<LeaderInfo>(&bytes) {
                        state.set_current_leader(Some(info));
                    }
                    // Bad payload: keep the session (the call worked) and the
                    // previous cached value.
                }
                // No leader / transport failure / timeout: drop the session so
                // the next tick redials (a timed-out call may have left an
                // orphaned in-flight request on the connection). Keep the
                // previous cached value — an absent leader during churn is
                // expected and the 409 body is best-effort.
                _ => session = None,
            }
        }
    }
}

/// Candidate diagnostics have no independent cache writer after this returns.
/// Catch only diagnostic polling: its session is discarded on panic and its
/// sole shared mutation is a completed atomic cache publication. The campaign
/// keeps its own shutdown/driver supervision and is never restarted.
async fn campaign_with_refresher(
    campaign: impl Future<Output = CampaignResult>,
    refresher: impl Future<Output = Infallible>,
) -> CampaignResult {
    tokio::pin!(campaign);
    let result = {
        let diagnostic = AssertUnwindSafe(refresher).catch_unwind();
        tokio::select! {
            result = &mut campaign => Some(result),
            result = diagnostic => match result {
                Ok(never) => match never {},
                Err(_) => None,
            },
        }
    };
    // The diagnostic future (including any pending cache publication) has been
    // dropped. Preserve the former spawned task's panic isolation without a
    // task that could publish a stale identity after the winner transition.
    match result {
        Some(result) => result,
        None => campaign.await,
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
#[path = "../../../../tests/unit/conf_mgr/conf_center/etcd/leader_election_test.rs"]
mod tests;
