//! Request-scoped AI quota batching.
//!
//! The response-body path only publishes cumulative output units into atomics.
//! One bounded task per quota-enabled AI request owns the timer and all Redis
//! I/O; final logging supplies the authoritative total for reconciliation.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tokio::task::JoinHandle;

mod runtime;

const FINAL_RETRY_DELAY: Duration = Duration::from_millis(10);
const FINAL_WAIT_GRACE: Duration = Duration::from_millis(50);

pub use runtime::{CompiledAiQuotaPlan, QuotaAdmission, QuotaExceededAction};

pub type QuotaFuture<'a> = Pin<Box<dyn Future<Output = Result<QuotaApplyResult, ()>> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaApplyResult {
    pub over_limit: bool,
}

/// Async execution seam kept separate from stream observation so the hot path
/// cannot accidentally perform Redis I/O.
pub trait QuotaSink: Send + Sync + 'static {
    fn apply_cumulative(&self, revision: u64, total: u64, final_update: bool) -> QuotaFuture<'_>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum QuotaFinalStatus {
    Charged = 1,
    OverLimit = 2,
    Failed = 3,
    TimedOut = 4,
    Unresolved = 5,
}

impl QuotaFinalStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Charged => "charged",
            Self::OverLimit => "over_limit",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Unresolved => "unresolved",
        }
    }

    fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Charged),
            2 => Some(Self::OverLimit),
            3 => Some(Self::Failed),
            4 => Some(Self::TimedOut),
            5 => Some(Self::Unresolved),
            _ => None,
        }
    }
}

pub struct QuotaAccumulator {
    observed_output: AtomicU64,
    final_total: AtomicU64,
    final_requested: AtomicBool,
    final_unknown: AtomicBool,
    completed: AtomicU8,
    /// Sticky "this request is over its limit" latch, set as soon as ANY
    /// flush — mid-stream or final — reports the cumulative total exceeded
    /// the configured limit. Kept separate from `completed`: that field is
    /// the terminal-status latch `wait_completed`/`finalize` block on, and
    /// overloading it here would make an in-progress request look finished.
    over_limit: AtomicBool,
    flush_tokens: u64,
    notify_worker: Notify,
    notify_completed: Notify,
}

impl QuotaAccumulator {
    // `pub(crate)`, not private: `ai::accounting::usage`'s tests construct a
    // `QuotaAccumulator` directly to exercise `over_limit()` without spinning
    // up the full quota worker.
    pub(crate) fn new(flush_tokens: u64) -> Self {
        Self {
            observed_output: AtomicU64::new(0),
            final_total: AtomicU64::new(0),
            final_requested: AtomicBool::new(false),
            final_unknown: AtomicBool::new(false),
            completed: AtomicU8::new(0),
            over_limit: AtomicBool::new(false),
            flush_tokens: flush_tokens.max(1),
            notify_worker: Notify::new(),
            notify_completed: Notify::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn observed_output_for_test(&self) -> u64 {
        self.observed_output.load(Ordering::Relaxed)
    }

    /// Add an approximate semantic output unit. The notification happens only
    /// when a configured batch boundary is crossed; atomics retain the full
    /// cumulative value when notifications coalesce.
    pub fn add_output_units(&self, delta: u64) {
        if delta == 0 {
            return;
        }
        let previous = self
            .observed_output
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_add(delta))
            })
            .unwrap_or(u64::MAX);
        let current = previous.saturating_add(delta);
        if previous / self.flush_tokens != current / self.flush_tokens {
            self.notify_worker.notify_one();
        }
    }

    /// Publish a provider-authoritative cumulative output value.
    pub fn observe_output_total(&self, total: u64) {
        let previous = self.observed_output.fetch_max(total, Ordering::Relaxed);
        if total > previous && previous / self.flush_tokens != total / self.flush_tokens {
            self.notify_worker.notify_one();
        }
    }

    fn request_final(&self, total: u64) {
        self.final_unknown.store(false, Ordering::Relaxed);
        self.final_total.store(total, Ordering::Relaxed);
        self.final_requested.store(true, Ordering::Release);
        self.notify_worker.notify_one();
    }

    fn request_final_unknown(&self) {
        self.final_unknown.store(true, Ordering::Relaxed);
        self.final_requested.store(true, Ordering::Release);
        self.notify_worker.notify_one();
    }

    // `pub(crate)` for the same reason as `new`: `ai::accounting::usage`'s test drives a
    // `QuotaAccumulator` straight to a final status without the worker.
    pub(crate) fn finish(&self, status: QuotaFinalStatus) {
        if status == QuotaFinalStatus::OverLimit {
            self.over_limit.store(true, Ordering::Release);
        }
        self.completed.store(status as u8, Ordering::Release);
        self.notify_completed.notify_waiters();
    }

    /// Marks the request over its limit. Called by `run_worker` as soon as
    /// any flush — mid-stream or final — reports the cumulative total
    /// exceeded the configured limit, so a long-running stream does not have
    /// to wait for final reconciliation before the body path can react.
    /// Sticky: safe to call more than once.
    pub(crate) fn mark_over_limit(&self) {
        self.over_limit.store(true, Ordering::Release);
    }

    /// Whether any quota flush (mid-stream or final) has reported this
    /// request over its limit. Non-blocking: a single Acquire atomic read,
    /// safe on the response body hot path.
    pub(crate) fn over_limit(&self) -> bool {
        self.over_limit.load(Ordering::Acquire)
    }

    async fn wait_completed(&self) -> QuotaFinalStatus {
        loop {
            let notified = self.notify_completed.notified();
            if let Some(status) = QuotaFinalStatus::from_u8(self.completed.load(Ordering::Acquire)) {
                return status;
            }
            notified.await;
        }
    }
}

/// Context-owned handle. Dropping it aborts the worker so a missing logging
/// callback cannot leave a detached timer task behind.
pub struct AiQuotaRequest {
    accumulator: Arc<QuotaAccumulator>,
    worker: Option<JoinHandle<()>>,
    final_timeout: Duration,
}

impl AiQuotaRequest {
    pub fn start(
        sink: Arc<dyn QuotaSink>,
        flush_tokens: u64,
        flush_interval: Duration,
        lease_refresh_interval: Duration,
        final_timeout: Duration,
    ) -> Self {
        let accumulator = Arc::new(QuotaAccumulator::new(flush_tokens));
        let worker_accumulator = Arc::clone(&accumulator);
        let worker = tokio::spawn(run_worker(
            worker_accumulator,
            sink,
            flush_interval,
            lease_refresh_interval,
            final_timeout,
        ));
        Self {
            accumulator,
            worker: Some(worker),
            final_timeout: final_timeout.saturating_add(FINAL_WAIT_GRACE),
        }
    }

    pub fn accumulator(&self) -> Arc<QuotaAccumulator> {
        Arc::clone(&self.accumulator)
    }

    pub async fn finalize(&mut self, authoritative_total: u64) -> QuotaFinalStatus {
        self.accumulator.request_final(authoritative_total);
        self.wait_final().await
    }

    pub(crate) async fn finalize_unknown(&mut self) -> QuotaFinalStatus {
        self.accumulator.request_final_unknown();
        self.wait_final().await
    }

    async fn wait_final(&mut self) -> QuotaFinalStatus {
        let status = match tokio::time::timeout(self.final_timeout, self.accumulator.wait_completed()).await {
            Ok(status) => status,
            Err(_) => {
                self.accumulator.finish(QuotaFinalStatus::TimedOut);
                QuotaFinalStatus::TimedOut
            }
        };
        if let Some(worker) = self.worker.take() {
            if status == QuotaFinalStatus::TimedOut {
                worker.abort();
            } else {
                let _ = worker.await;
            }
        }
        status
    }
}

impl Drop for AiQuotaRequest {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
    }
}

async fn run_worker(
    accumulator: Arc<QuotaAccumulator>,
    sink: Arc<dyn QuotaSink>,
    flush_interval: Duration,
    lease_refresh_interval: Duration,
    final_timeout: Duration,
) {
    let start = tokio::time::Instant::now() + flush_interval;
    let mut interval = tokio::time::interval_at(start, flush_interval);
    let lease_start = tokio::time::Instant::now() + lease_refresh_interval;
    let mut lease_interval = tokio::time::interval_at(lease_start, lease_refresh_interval);
    let mut revision = 0_u64;
    let mut charged_total = 0_u64;
    // A failed or timed-out apply may have committed in one or more Redis buckets.
    // Record attempted targets before awaiting, independently of acknowledgements.
    let mut sent_high_water = 0_u64;
    let mut final_deadline = None;
    let mut retry_final = false;
    let mut has_charge = false;

    loop {
        let refresh_lease = if retry_final {
            tokio::time::sleep(FINAL_RETRY_DELAY).await;
            false
        } else {
            tokio::select! {
                _ = interval.tick() => false,
                _ = lease_interval.tick() => true,
                _ = accumulator.notify_worker.notified() => false,
            }
        };

        let final_update = accumulator.final_requested.load(Ordering::Acquire);
        let unknown_final = final_update && accumulator.final_unknown.load(Ordering::Relaxed);
        let target = if final_update {
            final_deadline.get_or_insert_with(|| tokio::time::Instant::now() + final_timeout);
            if unknown_final {
                accumulator
                    .observed_output
                    .load(Ordering::Relaxed)
                    .max(charged_total)
                    .max(sent_high_water)
            } else {
                accumulator.final_total.load(Ordering::Relaxed)
            }
        } else {
            accumulator.observed_output.load(Ordering::Relaxed)
        };
        if target == charged_total && !final_update && !(refresh_lease && has_charge) {
            continue;
        }

        revision = revision.saturating_add(1);
        sent_high_water = sent_high_water.max(target);
        match sink.apply_cumulative(revision, target, final_update).await {
            Ok(result) => {
                charged_total = target;
                retry_final = false;
                has_charge = true;
                if result.over_limit {
                    // Publish as soon as this flush sees it, mid-stream or
                    // final: the response body path polls this instead of
                    // waiting for `finalize`'s end-of-stream reconciliation.
                    accumulator.mark_over_limit();
                }
                lease_interval.reset_after(lease_refresh_interval);
                if final_update {
                    accumulator.finish(if unknown_final {
                        QuotaFinalStatus::Unresolved
                    } else if result.over_limit {
                        QuotaFinalStatus::OverLimit
                    } else {
                        QuotaFinalStatus::Charged
                    });
                    return;
                }
            }
            Err(()) if final_update => {
                let deadline = final_deadline.expect("final deadline is set with final request");
                if tokio::time::Instant::now() + FINAL_RETRY_DELAY >= deadline {
                    accumulator.finish(QuotaFinalStatus::Failed);
                    return;
                }
                retry_final = true;
            }
            Err(()) => {
                // Keep the watermark unchanged. The cumulative idempotent
                // operation is retried on the next timer tick.
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/ai/quota/mod_test.rs"]
mod tests;
