//! Shared background health-probe driver for LinkSys data sinks (task 05).
//!
//! Provides a generic `run_probe_loop` that periodically runs a provider-supplied
//! liveness closure and drives the provider's `healthy` flag via CAS, plus the
//! `apply_health_result` helper reused by the ES bulk path so every `healthy`
//! writer flips through the same win-gated emission (metric + rate-limited log).
//!
//! Constants mirror the etcd monitor (`edgion-common/.../etcd/client.rs:160-242`).

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use rand::Rng;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::observe::metrics::registry::{record_link_sys_health, record_link_sys_health_flip};

/// Canonical LinkSys flip-log rate-limit window.
const FLIP_LOG_WINDOW_MS: u64 = 5_000;

/// Process-start monotonic base for LinkSys flip-log rate limiting.
static FLIP_CLOCK_BASE: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Deterministic FAILURE backoff state: start 500ms, double, clamp at `max`.
/// The healthy-cadence interval (10s) is NOT here — it lives in `ProbeConfig`.
pub(crate) struct ProbeBackoff {
    start: Duration,
    cur: Duration,
    max: Duration,
}

impl ProbeBackoff {
    pub(crate) fn new(start: Duration, max: Duration) -> Self {
        Self { start, cur: start, max }
    }

    /// Reset after a successful probe (next interval becomes `base_interval`).
    pub(crate) fn on_success(&mut self) {
        self.cur = self.start;
    }

    /// Return the jittered next interval, then advance the backoff. `jitter` is a
    /// multiplier (e.g. 0.8..=1.2); only the returned wait is jittered, `cur`
    /// doubles deterministically — matching the etcd monitor.
    pub(crate) fn on_failure(&mut self, jitter: f64) -> Duration {
        let next = self.cur.mul_f64(jitter).min(self.max);
        self.cur = (self.cur * 2).min(self.max);
        next
    }
}

/// Best-effort flip-log rate-limit gate (clock injected for tests). Returns true
/// and stamps `last` iff this is the first flip (`last == 0`) or `window_ms` has
/// elapsed. Never stores the 0 sentinel.
///
/// Non-atomic: the load/store pair is not a single CAS, so concurrent first callers
/// may both return `true` (a one-off double-log). This is acceptable — the gate only
/// limits log volume; it is not a correctness barrier.
fn flip_log_allowed_at(last: &AtomicU64, now_ms: u64, window_ms: u64) -> bool {
    let prev = last.load(Ordering::Relaxed);
    if prev != 0 && now_ms.saturating_sub(prev) < window_ms {
        return false;
    }
    last.store(now_ms.max(1), Ordering::Relaxed);
    true
}

/// Apply the canonical LinkSys flip-log gate using the real monotonic clock.
pub(crate) fn flip_log_allowed(last: &AtomicU64) -> bool {
    let now_ms = Instant::now().saturating_duration_since(*FLIP_CLOCK_BASE).as_millis() as u64;
    flip_log_allowed_at(last, now_ms, FLIP_LOG_WINDOW_MS)
}

/// Apply one probe/operation result to `healthy` via CAS, emitting the gauge +
/// flip counter + a rate-limited log ONLY on a winning transition. `upgrade_only`
/// suppresses the true->false direction (the data path owns downgrade). Returns
/// true iff a flip was emitted. This is the single coordination point that keeps
/// the flip counter accurate when two writers share one flag.
pub(crate) fn apply_health_result(
    healthy: &AtomicBool,
    name: &str,
    system_type: &str,
    last_flip_log: &AtomicU64,
    ok: bool,
    upgrade_only: bool,
) -> bool {
    if ok {
        if healthy
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            record_link_sys_health(name, true);
            record_link_sys_health_flip(name, true);
            if flip_log_allowed(last_flip_log) {
                tracing::info!(link = %name, system = %system_type, "link-sys recovered");
            }
            return true;
        }
    } else if !upgrade_only
        && healthy
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        record_link_sys_health(name, false);
        record_link_sys_health_flip(name, false);
        if flip_log_allowed(last_flip_log) {
            tracing::warn!(link = %name, system = %system_type, "link-sys degraded");
        }
        return true;
    }
    false
}

/// Per-probe configuration. Intervals are hard-coded by callers (no CRD fields).
pub(crate) struct ProbeConfig {
    pub name: String,
    pub system_type: &'static str,
    pub base_interval: Duration,
    pub max_backoff: Duration,
    pub probe_timeout: Duration,
    /// ES = true (probe only recovers; the bulk path owns downgrade).
    /// Kafka/OTLP = false (probe is authoritative in both directions).
    pub upgrade_only: bool,
}

/// Background probe loop. Standalone async fn (no internal spawn) so tests can
/// await it directly under `start_paused`. Exits on a shutdown send OR when all
/// `watch` senders drop (the OTLP latent-shutdown path).
///
/// Structure: probe-then-wait. The first probe fires immediately on entry; subsequent
/// probes run after `base_interval` (healthy) or a jittered backoff (unhealthy).
/// Tests pair `advance(base_interval + ε)` with `yield_now`; because the probe fires
/// before the wait, `advance` only needs to cover the sleep — no cold-start window.
pub(crate) async fn run_probe_loop<F, Fut>(
    cfg: ProbeConfig,
    healthy: Arc<AtomicBool>,
    mut shutdown_rx: watch::Receiver<bool>,
    probe_fn: F,
) where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = bool> + Send + 'static,
{
    let mut backoff = ProbeBackoff::new(Duration::from_millis(500), cfg.max_backoff);
    let last_flip_log = AtomicU64::new(0);

    loop {
        // The closure is bounded here; a timeout counts as unhealthy. This is
        // load-bearing for Kafka, whose internal backoff never returns Err.
        let ok = matches!(tokio::time::timeout(cfg.probe_timeout, probe_fn()).await, Ok(true));

        apply_health_result(
            &healthy,
            &cfg.name,
            cfg.system_type,
            &last_flip_log,
            ok,
            cfg.upgrade_only,
        );

        // Compute the wait for the NEXT iteration before suspending.
        let interval = if ok {
            backoff.on_success();
            cfg.base_interval
        } else {
            // +/-20% jitter so N pods sharing a backend don't reconnect in lockstep.
            let jitter = rand::rng().random_range(0.8_f64..=1.2);
            backoff.on_failure(jitter)
        };

        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = shutdown_rx.changed() => {
                // Ok(()) = shutdown signaled; Err = all senders dropped. Both mean stop.
                tracing::info!(link = %cfg.name, system = %cfg.system_type, "health probe shutting down");
                return;
            }
        }
    }
}

/// Spawn `run_probe_loop` as a detached task. Returns the handle; ES/Kafka store
/// and `abort()` it on shutdown. OTLP lets the handle detach and instead relies on
/// the shutdown watch sender (stored on the caller, dropped with the struct) to
/// signal the loop to exit.
pub(crate) fn spawn_health_probe<F, Fut>(
    cfg: ProbeConfig,
    healthy: Arc<AtomicBool>,
    shutdown_rx: watch::Receiver<bool>,
    probe_fn: F,
) -> JoinHandle<()>
where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = bool> + Send + 'static,
{
    tokio::spawn(run_probe_loop(cfg, healthy, shutdown_rx, probe_fn))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_resets_on_success() {
        let mut b = ProbeBackoff::new(Duration::from_millis(500), Duration::from_secs(30));
        let _ = b.on_failure(1.0); // cur now 1s
        b.on_success();
        // After reset, the first failure interval is back to ~start.
        assert_eq!(b.on_failure(1.0), Duration::from_millis(500));
    }

    #[test]
    fn backoff_doubles_and_clamps() {
        let mut b = ProbeBackoff::new(Duration::from_secs(10), Duration::from_secs(30));
        assert_eq!(b.on_failure(1.0), Duration::from_secs(10)); // returns cur, then cur=20
        assert_eq!(b.on_failure(1.0), Duration::from_secs(20)); // returns 20, then cur=30 (clamp)
        assert_eq!(b.on_failure(1.0), Duration::from_secs(30)); // clamped
        assert_eq!(b.on_failure(1.0), Duration::from_secs(30)); // stays clamped
                                                                // jitter > 1.0 at the clamped ceiling: returned interval must not exceed max
        assert_eq!(
            b.on_failure(1.2),
            Duration::from_secs(30),
            "jitter must not push returned interval past max"
        );
    }

    #[test]
    fn backoff_applies_jitter_to_returned_interval_only() {
        let mut b = ProbeBackoff::new(Duration::from_secs(10), Duration::from_secs(30));
        let lo = b.on_failure(0.8);
        assert_eq!(lo, Duration::from_secs(8)); // 10 * 0.8, cur now 20
        let hi = b.on_failure(1.2);
        assert_eq!(hi, Duration::from_secs(24)); // 20 * 1.2, cur now 30
    }

    #[test]
    fn flip_log_window_rate_limits() {
        let last = AtomicU64::new(0);
        assert!(flip_log_allowed_at(&last, 1_000, 5_000)); // first flip always logs
        assert!(!flip_log_allowed_at(&last, 2_000, 5_000)); // within window suppressed
        assert!(flip_log_allowed_at(&last, 6_000, 5_000)); // window elapsed
        assert_ne!(last.load(Ordering::Relaxed), 0); // never stores the sentinel
    }

    #[test]
    fn flip_log_first_call_at_time_zero_logs() {
        let last = AtomicU64::new(0);
        assert!(flip_log_allowed_at(&last, 0, 5_000));
        assert_eq!(last.load(Ordering::Relaxed), 1); // now_ms.max(1)
    }

    #[test]
    fn flip_log_clock_rollback_waits_for_the_full_window_after_catch_up() {
        let last = AtomicU64::new(0);
        assert!(flip_log_allowed_at(&last, 10_000, 5_000));

        assert!(!flip_log_allowed_at(&last, 9_000, 5_000));
        assert_eq!(last.load(Ordering::Relaxed), 10_000);

        assert!(!flip_log_allowed_at(&last, 10_000, 5_000));
        assert!(flip_log_allowed_at(&last, 15_000, 5_000));
    }

    #[test]
    fn apply_result_flips_only_on_transition() {
        let h = AtomicBool::new(false);
        let last = AtomicU64::new(0);
        assert!(apply_health_result(&h, "n", "test", &last, true, false)); // false->true
        assert!(h.load(Ordering::Relaxed));
        assert!(!apply_health_result(&h, "n", "test", &last, true, false)); // already true, no flip
        assert!(apply_health_result(&h, "n", "test", &last, false, false)); // true->false
        assert!(!h.load(Ordering::Relaxed));
    }

    #[test]
    fn apply_result_single_winner_under_contention() {
        use std::sync::Arc;
        let h = Arc::new(AtomicBool::new(false));
        let last = Arc::new(AtomicU64::new(0));
        let mut handles = Vec::new();
        for _ in 0..16 {
            let h = Arc::clone(&h);
            let last = Arc::clone(&last);
            handles.push(std::thread::spawn(move || {
                apply_health_result(&h, "n", "test", &last, true, false)
            }));
        }
        let winners = handles.into_iter().map(|h| h.join().unwrap()).filter(|&w| w).count();
        assert_eq!(winners, 1, "exactly one thread may win the false->true flip");
    }

    #[test]
    fn apply_result_upgrade_only_never_downgrades() {
        let h = AtomicBool::new(true);
        let last = AtomicU64::new(0);
        // upgrade_only: a failure must NOT flip the flag to false.
        assert!(!apply_health_result(&h, "n", "test", &last, false, true));
        assert!(h.load(Ordering::Relaxed));
        // but it can still recover a false flag.
        h.store(false, Ordering::Relaxed);
        assert!(apply_health_result(&h, "n", "test", &last, true, true));
        assert!(h.load(Ordering::Relaxed));
    }

    #[tokio::test(start_paused = true)]
    async fn run_probe_loop_drives_flag_and_stops_on_shutdown() {
        use std::sync::atomic::AtomicUsize;
        // Scripted closure: call 1 -> true (recover), call 2 -> false (down),
        // call 3+ -> true. upgrade_only=false so it can both up- and downgrade.
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_c = Arc::clone(&calls);
        let probe = move || {
            let calls = Arc::clone(&calls_c);
            async move {
                let n = calls.fetch_add(1, Ordering::Relaxed);
                n != 1 // call index 1 (the 2nd call) reports unhealthy
            }
        };

        let healthy = Arc::new(AtomicBool::new(false));
        let (sd_tx, sd_rx) = watch::channel(false);
        let cfg = ProbeConfig {
            name: "t".into(),
            system_type: "test",
            base_interval: Duration::from_secs(10),
            max_backoff: Duration::from_secs(30),
            probe_timeout: Duration::from_secs(5),
            upgrade_only: false,
        };
        let h = Arc::clone(&healthy);
        let task = tokio::spawn(run_probe_loop(cfg, h, sd_rx, probe));

        // Advance past the first interval -> recover.
        tokio::time::advance(Duration::from_secs(11)).await;
        tokio::task::yield_now().await;
        assert!(healthy.load(Ordering::Relaxed), "first probe should recover");

        // Next interval -> unhealthy.
        tokio::time::advance(Duration::from_secs(11)).await;
        tokio::task::yield_now().await;
        assert!(!healthy.load(Ordering::Relaxed), "second probe should downgrade");

        // Shutdown -> loop returns cleanly (no leak).
        sd_tx.send(true).unwrap();
        tokio::time::advance(Duration::from_secs(1)).await;
        let joined = tokio::time::timeout(Duration::from_secs(1), task).await;
        assert!(joined.is_ok(), "probe loop must exit on shutdown");
    }

    #[tokio::test(start_paused = true)]
    async fn run_probe_loop_exits_when_sender_dropped() {
        let healthy = Arc::new(AtomicBool::new(true));
        let (sd_tx, sd_rx) = watch::channel(false);
        let cfg = ProbeConfig {
            name: "t".into(),
            system_type: "test",
            base_interval: Duration::from_secs(10),
            max_backoff: Duration::from_secs(30),
            probe_timeout: Duration::from_secs(5),
            upgrade_only: false,
        };
        let task = tokio::spawn(run_probe_loop(cfg, healthy, sd_rx, || async { true }));
        drop(sd_tx); // OTLP latent-shutdown path: all senders gone.
        tokio::task::yield_now().await; // give the task CPU to observe Err on changed()
        let joined = tokio::time::timeout(Duration::from_secs(1), task).await;
        assert!(joined.is_ok(), "probe loop must exit when the watch sender drops");
    }
}
