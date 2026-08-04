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
    flush_tokens: u64,
    notify_worker: Notify,
    notify_completed: Notify,
}

impl QuotaAccumulator {
    fn new(flush_tokens: u64) -> Self {
        Self {
            observed_output: AtomicU64::new(0),
            final_total: AtomicU64::new(0),
            final_requested: AtomicBool::new(false),
            completed: AtomicU8::new(0),
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

    fn finish(&self, status: QuotaFinalStatus) {
        self.completed.store(status as u8, Ordering::Release);
        self.notify_completed.notify_waiters();
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
