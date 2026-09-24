//! Leader state shared between the election supervisor, the lifecycle gate,
//! the Admin API 409 gate, and the pre-write fence (spec §4.6, §6.2).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwapOption;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeaderInfo {
    pub identity: String,
    pub grpc_addr: String,
    pub admin_addr: String,
}

pub struct LeaderState {
    /// Current leadership flag, published as a `watch` channel so process-wide
    /// singleton components (the federation client supervisor) can subscribe to
    /// edges instead of polling. The sender is the authoritative storage; there
    /// is no separate atomic.
    is_leader: Arc<watch::Sender<bool>>,
    last_ok_mono_millis: AtomicU64, // millis since a process-start baseline
    last_ok_wall_millis: AtomicU64, // unix millis
    lease_duration: Duration,
    current_leader: ArcSwapOption<LeaderInfo>,
    baseline: Instant,
}

pub fn wall_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl LeaderState {
    pub fn new(lease_duration: Duration) -> Arc<Self> {
        Arc::new(Self {
            is_leader: Arc::new(watch::channel(false).0),
            last_ok_mono_millis: AtomicU64::new(0),
            last_ok_wall_millis: AtomicU64::new(0),
            lease_duration,
            current_leader: ArcSwapOption::from(None),
            baseline: Instant::now(),
        })
    }
    pub fn set_leader(&self, v: bool) {
        // `send_replace` never fails when there are no receivers, unlike `send`.
        self.is_leader.send_replace(v);
    }
    pub fn is_leader(&self) -> bool {
        *self.is_leader.borrow()
    }
    /// Subscribe to the leadership flag. See `CenterLifeCycle::leader_signal`.
    pub fn subscribe_leader(&self) -> watch::Receiver<bool> {
        self.is_leader.subscribe()
    }
    pub fn record_keepalive_now(&self) {
        self.record_keepalive_at(Instant::now(), wall_now());
    }
    pub fn record_keepalive_at(&self, mono: Instant, wall: u64) {
        // Clamp to >= 1 so 0 stays reserved as the "never recorded / invalidated"
        // sentinel: a keepalive taken within 1ms of the process-start baseline
        // would otherwise round to 0 and be indistinguishable from "never recorded".
        let mono_millis = (mono.saturating_duration_since(self.baseline).as_millis() as u64).max(1);
        self.last_ok_mono_millis.store(mono_millis, Ordering::Release);
        self.last_ok_wall_millis.store(wall, Ordering::Release);
    }
    pub fn mark_lease_invalid(&self) {
        self.last_ok_mono_millis.store(0, Ordering::Release);
        self.last_ok_wall_millis.store(0, Ordering::Release);
    }
    pub fn set_current_leader(&self, info: Option<LeaderInfo>) {
        self.current_leader.store(info.map(Arc::new));
    }
    pub fn current_leader(&self) -> Option<LeaderInfo> {
        self.current_leader.load().as_ref().map(|a| (**a).clone())
    }
    /// Spec §4.6: gate on max(monotonic, wall) elapsed so a VM suspend
    /// (which freezes CLOCK_MONOTONIC) cannot make a stale lease look fresh.
    fn lease_is_fresh(&self) -> bool {
        let now_mono = Instant::now().saturating_duration_since(self.baseline).as_millis() as u64;
        let last_mono = self.last_ok_mono_millis.load(Ordering::Acquire);
        let last_wall = self.last_ok_wall_millis.load(Ordering::Acquire);
        if last_mono == 0 || last_wall == 0 {
            return false;
        }
        let mono_elapsed = now_mono.saturating_sub(last_mono);
        let wall_elapsed = wall_now().saturating_sub(last_wall);
        let elapsed = mono_elapsed.max(wall_elapsed);
        elapsed < self.lease_duration.as_millis() as u64
    }

    /// Time since the last successful keepalive by max(monotonic, wall).
    /// Returns a very large duration when no keepalive has been recorded
    /// (or the lease was invalidated), so callers treat it as "expired".
    pub fn elapsed_since_last_ok(&self) -> Duration {
        let now_mono = Instant::now().saturating_duration_since(self.baseline).as_millis() as u64;
        let last_mono = self.last_ok_mono_millis.load(Ordering::Acquire);
        let last_wall = self.last_ok_wall_millis.load(Ordering::Acquire);
        if last_mono == 0 || last_wall == 0 {
            return Duration::from_secs(u64::MAX / 2);
        }
        let mono = now_mono.saturating_sub(last_mono);
        let wall = wall_now().saturating_sub(last_wall);
        Duration::from_millis(mono.max(wall))
    }
}

#[derive(Clone)]
pub struct LeaderHandle {
    inner: Arc<LeaderState>,
}
impl LeaderHandle {
    pub fn from_state(inner: Arc<LeaderState>) -> Self {
        Self { inner }
    }
    pub fn is_leader(&self) -> bool {
        self.inner.is_leader()
    }
    /// Live leadership signal for process-wide singleton components
    /// (see `CenterLifeCycle::leader_signal`). Deliberately NOT composed with
    /// lease freshness — that stricter composition belongs to `LeadershipGate`.
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.inner.subscribe_leader()
    }
    pub fn current_leader(&self) -> Option<LeaderInfo> {
        self.inner.current_leader()
    }
}

#[derive(Debug)]
pub struct NotLeader;

#[derive(Clone)]
pub struct LeadershipGate {
    inner: Arc<LeaderState>,
}
impl LeadershipGate {
    pub fn from_state(inner: Arc<LeaderState>) -> Self {
        Self { inner }
    }
    /// Ok only if we are leader AND the lease is fresh by max(mono, wall).
    pub fn check(&self) -> Result<(), NotLeader> {
        if self.inner.is_leader() && self.inner.lease_is_fresh() {
            Ok(())
        } else {
            Err(NotLeader)
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/etcd/leadership_gate_test.rs"]
mod tests;
