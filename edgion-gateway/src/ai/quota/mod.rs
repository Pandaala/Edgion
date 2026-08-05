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
}

impl QuotaFinalStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Charged => "charged",
            Self::OverLimit => "over_limit",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
        }
    }

    fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Charged),
            2 => Some(Self::OverLimit),
            3 => Some(Self::Failed),
            4 => Some(Self::TimedOut),
            _ => None,
        }
    }
}

pub struct QuotaAccumulator {
    observed_output: AtomicU64,
    final_total: AtomicU64,
    final_requested: AtomicBool,
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
    // `pub(crate)`, not private: `ai::usage`'s tests construct a
    // `QuotaAccumulator` directly to exercise `over_limit()` without spinning
    // up the full quota worker.
    pub(crate) fn new(flush_tokens: u64) -> Self {
        Self {
            observed_output: AtomicU64::new(0),
            final_total: AtomicU64::new(0),
            final_requested: AtomicBool::new(false),
            completed: AtomicU8::new(0),
            over_limit: AtomicBool::new(false),
            flush_tokens: flush_tokens.max(1),
            notify_worker: Notify::new(),
            notify_completed: Notify::new(),
        }
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
        self.final_total.store(total, Ordering::Relaxed);
        self.final_requested.store(true, Ordering::Release);
        self.notify_worker.notify_one();
    }

    // `pub(crate)` for the same reason as `new`: `ai::usage`'s test drives a
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
        let target = if final_update {
            final_deadline.get_or_insert_with(|| tokio::time::Instant::now() + final_timeout);
            accumulator.final_total.load(Ordering::Relaxed)
        } else {
            accumulator.observed_output.load(Ordering::Relaxed)
        };
        if target == charged_total && !final_update && !(refresh_lease && has_charge) {
            continue;
        }

        revision = revision.saturating_add(1);
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
                    accumulator.finish(if result.over_limit {
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
mod tests {
    use super::*;
    use parking_lot::Mutex;

    struct RecordingSink {
        calls: Mutex<Vec<(u64, u64, bool)>>,
        final_failures_remaining: AtomicU64,
        notify: Notify,
    }

    impl RecordingSink {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                final_failures_remaining: AtomicU64::new(0),
                notify: Notify::new(),
            }
        }

        fn fail_next_final_calls(&self, count: u64) {
            self.final_failures_remaining.store(count, Ordering::Relaxed);
        }

        async fn wait_for_calls(&self, count: usize) {
            loop {
                let notified = self.notify.notified();
                if self.calls.lock().len() >= count {
                    return;
                }
                notified.await;
            }
        }
    }

    impl QuotaSink for RecordingSink {
        fn apply_cumulative(&self, revision: u64, total: u64, final_update: bool) -> QuotaFuture<'_> {
            Box::pin(async move {
                self.calls.lock().push((revision, total, final_update));
                self.notify.notify_waiters();
                if final_update
                    && self
                        .final_failures_remaining
                        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                            remaining.checked_sub(1)
                        })
                        .is_ok()
                {
                    return Err(());
                }
                Ok(QuotaApplyResult {
                    over_limit: total > 100,
                })
            })
        }
    }

    #[tokio::test]
    async fn threshold_notifications_coalesce_and_final_reconciles_downward() {
        let sink = Arc::new(RecordingSink::new());
        let sink_trait: Arc<dyn QuotaSink> = sink.clone();
        let mut request = AiQuotaRequest::start(
            sink_trait,
            10,
            Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_secs(1),
        );
        let accumulator = request.accumulator();
        accumulator.add_output_units(4);
        tokio::task::yield_now().await;
        assert!(sink.calls.lock().is_empty());
        accumulator.add_output_units(16);
        sink.wait_for_calls(1).await;
        assert_eq!(sink.calls.lock()[0], (1, 20, false));

        assert_eq!(request.finalize(17).await, QuotaFinalStatus::Charged);
        assert_eq!(sink.calls.lock()[1], (2, 17, true));
    }

    #[tokio::test(start_paused = true)]
    async fn timer_flushes_a_partial_batch() {
        let sink = Arc::new(RecordingSink::new());
        let sink_trait: Arc<dyn QuotaSink> = sink.clone();
        let mut request = AiQuotaRequest::start(
            sink_trait,
            100,
            Duration::from_secs(1),
            Duration::from_secs(60),
            Duration::from_secs(1),
        );
        request.accumulator().add_output_units(3);
        tokio::time::advance(Duration::from_secs(1)).await;
        sink.wait_for_calls(1).await;
        assert_eq!(sink.calls.lock()[0], (1, 3, false));
        assert_eq!(request.finalize(3).await, QuotaFinalStatus::Charged);
    }

    #[tokio::test]
    async fn final_over_limit_is_reported() {
        let sink = Arc::new(RecordingSink::new());
        let sink_trait: Arc<dyn QuotaSink> = sink;
        let mut request = AiQuotaRequest::start(
            sink_trait,
            10,
            Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_secs(1),
        );
        assert_eq!(request.finalize(101).await, QuotaFinalStatus::OverLimit);
    }

    #[tokio::test]
    async fn final_reconciliation_retries_within_one_total_deadline() {
        let sink = Arc::new(RecordingSink::new());
        sink.fail_next_final_calls(1);
        let sink_trait: Arc<dyn QuotaSink> = sink.clone();
        let mut request = AiQuotaRequest::start(
            sink_trait,
            10,
            Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_millis(100),
        );

        assert_eq!(request.finalize(17).await, QuotaFinalStatus::Charged);
        assert_eq!(sink.calls.lock().as_slice(), &[(1, 17, true), (2, 17, true)]);
    }

    #[tokio::test]
    async fn final_reconciliation_reports_failure_after_retry_budget() {
        let sink = Arc::new(RecordingSink::new());
        sink.fail_next_final_calls(u64::MAX);
        let sink_trait: Arc<dyn QuotaSink> = sink.clone();
        let mut request = AiQuotaRequest::start(
            sink_trait,
            10,
            Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_millis(35),
        );

        assert_eq!(request.finalize(17).await, QuotaFinalStatus::Failed);
        assert!(sink.calls.lock().len() >= 2);
    }

    #[test]
    fn over_limit_reports_the_final_status_without_blocking() {
        let acc = QuotaAccumulator::new(16);
        assert!(!acc.over_limit());
        acc.finish(QuotaFinalStatus::Charged);
        assert!(!acc.over_limit());

        let acc = QuotaAccumulator::new(16);
        acc.finish(QuotaFinalStatus::OverLimit);
        assert!(acc.over_limit());
    }

    /// Regression coverage for the dead-code bug where `over_limit()` could
    /// only ever become true through `finalize`'s end-of-stream
    /// reconciliation, days after the body had already streamed. This test
    /// never calls `finish` or `finalize` at all: it drives the accumulator
    /// exactly the way production does — through `run_worker`'s mid-stream
    /// flush — and proves `over_limit()` flips before any final
    /// reconciliation happens.
    #[tokio::test]
    async fn mid_stream_flush_publishes_over_limit_before_final_reconciliation() {
        let sink = Arc::new(RecordingSink::new());
        let sink_trait: Arc<dyn QuotaSink> = sink.clone();
        let mut request = AiQuotaRequest::start(
            sink_trait,
            10,
            Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_secs(1),
        );
        let accumulator = request.accumulator();
        assert!(!accumulator.over_limit());

        // RecordingSink reports over_limit for any total > 100.
        accumulator.add_output_units(150);
        sink.wait_for_calls(1).await;

        assert!(
            accumulator.over_limit(),
            "a mid-stream flush must publish over-limit without waiting for finalize"
        );
        assert_eq!(
            sink.calls.lock()[0],
            (1, 150, false),
            "the flush that reported it was non-final"
        );

        // Marking over-limit early must not short-circuit the final
        // settlement the logging phase still depends on.
        assert_eq!(request.finalize(150).await, QuotaFinalStatus::OverLimit);
    }

    #[tokio::test(start_paused = true)]
    async fn sparse_lease_heartbeats_keep_an_active_request_reconcilable() {
        let sink = Arc::new(RecordingSink::new());
        let sink_trait: Arc<dyn QuotaSink> = sink.clone();
        let mut request = AiQuotaRequest::start(
            sink_trait,
            1,
            Duration::from_secs(60),
            Duration::from_millis(100),
            Duration::from_secs(1),
        );
        request.accumulator().add_output_units(1);
        sink.wait_for_calls(1).await;

        tokio::time::advance(Duration::from_millis(350)).await;
        sink.wait_for_calls(4).await;
        assert_eq!(
            sink.calls.lock()[..4],
            [(1, 1, false), (2, 1, false), (3, 1, false), (4, 1, false)]
        );

        assert_eq!(request.finalize(2).await, QuotaFinalStatus::Charged);
        assert_eq!(sink.calls.lock().last(), Some(&(5, 2, true)));
    }
}
