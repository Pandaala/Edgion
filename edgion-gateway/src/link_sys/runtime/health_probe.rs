//! Shared background health-probe driver for LinkSys data sinks (task 05).
//!
//! Provides a generic `run_probe_loop` that periodically runs a provider-supplied
//! liveness closure and drives the provider's publication-aware health cell,
//! plus the `apply_health_result` helper reused by provider operation paths.
//!
//! Constants mirror the etcd monitor (`edgion-common/.../etcd/client.rs:160-242`).

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use rand::Rng;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use edgion_common::link_sys_core::{HealthTransitionReporter, PublishedHealth};

use crate::observe::metrics::registry::record_link_sys_health_flip;

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

/// Build the binary-owned reporter injected into each Gateway generation.
pub(crate) fn health_transition_reporter(name: &str) -> HealthTransitionReporter {
    let name = name.to_string();
    Arc::new(move |healthy| record_link_sys_health_flip(&name, healthy))
}

/// Build an unpublished Gateway health cell. The persistent runner activates it
/// only when the owning generation becomes current.
pub(crate) fn new_published_health(name: &str, initial: bool) -> Arc<PublishedHealth> {
    Arc::new(PublishedHealth::with_reporter(
        initial,
        health_transition_reporter(name),
    ))
}

/// Apply one probe/operation result through the canonical health cell, emitting
/// a rate-limited log only on a real transition. The cell itself decides whether
/// the binary-owned flip reporter is active for the current generation.
pub(crate) fn apply_health_result(
    healthy: &PublishedHealth,
    name: &str,
    system_type: &str,
    last_flip_log: &AtomicU64,
    ok: bool,
    upgrade_only: bool,
) -> bool {
    if !healthy.update(ok, upgrade_only) {
        return false;
    }
    if flip_log_allowed(last_flip_log) {
        if ok {
            tracing::info!(link = %name, system = %system_type, "link-sys recovered");
        } else {
            tracing::warn!(link = %name, system = %system_type, "link-sys degraded");
        }
    }
    true
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
    healthy: Arc<PublishedHealth>,
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
        if *shutdown_rx.borrow() {
            return;
        }
        let ok = tokio::select! {
            biased;
            _ = shutdown_rx.changed() => return,
            result = tokio::time::timeout(cfg.probe_timeout, probe_fn()) => matches!(result, Ok(true)),
        };

        // A shutdown that lands after the probe future resolves must not
        // update a retired generation's health state or flip counter.
        if *shutdown_rx.borrow() {
            return;
        }

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

/// Spawn `run_probe_loop` as a detached task. Returns the handle; ES, Kafka, and
/// OTLP `abort()` it from `mark_retired` so a retired generation stops probing
/// after handover. The shutdown watch remains a best-effort
/// clean loop exit when abort races the next select.
pub(crate) fn spawn_health_probe<F, Fut>(
    cfg: ProbeConfig,
    healthy: Arc<PublishedHealth>,
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
#[path = "../../../tests/unit/link_sys/runtime/health_probe_test.rs"]
mod tests;
