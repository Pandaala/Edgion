use super::*;
use parking_lot::Mutex;

struct RecordingSink {
    calls: Mutex<Vec<(u64, u64, bool)>>,
    failures_remaining: AtomicU64,
    notify: Notify,
}

impl RecordingSink {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            failures_remaining: AtomicU64::new(0),
            notify: Notify::new(),
        }
    }

    fn fail_next_calls(&self, count: u64) {
        self.failures_remaining.store(count, Ordering::Relaxed);
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
            if self
                .failures_remaining
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

struct HangingSink {
    calls: AtomicU64,
    cancelled: AtomicU64,
    notify: Notify,
}

impl HangingSink {
    fn new() -> Self {
        Self {
            calls: AtomicU64::new(0),
            cancelled: AtomicU64::new(0),
            notify: Notify::new(),
        }
    }

    async fn wait_for_call(&self) {
        loop {
            let notified = self.notify.notified();
            if self.calls.load(Ordering::Acquire) > 0 {
                return;
            }
            notified.await;
        }
    }
}

struct CancellationGuard<'a>(&'a AtomicU64);

impl Drop for CancellationGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Release);
    }
}

impl QuotaSink for HangingSink {
    fn apply_cumulative(&self, _revision: u64, _total: u64, _final_update: bool) -> QuotaFuture<'_> {
        Box::pin(async move {
            let _guard = CancellationGuard(&self.cancelled);
            self.calls.fetch_add(1, Ordering::Release);
            self.notify.notify_waiters();
            std::future::pending().await
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

#[tokio::test(start_paused = true)]
async fn failed_incremental_flush_retries_the_same_watermark() {
    let sink = Arc::new(RecordingSink::new());
    sink.fail_next_calls(1);
    let sink_trait: Arc<dyn QuotaSink> = sink.clone();
    let mut request = AiQuotaRequest::start(
        sink_trait,
        10,
        Duration::from_secs(1),
        Duration::from_secs(60),
        Duration::from_secs(1),
    );
    request.accumulator().add_output_units(20);
    sink.wait_for_calls(1).await;
    assert_eq!(sink.calls.lock()[0], (1, 20, false));

    tokio::time::advance(Duration::from_secs(1)).await;
    sink.wait_for_calls(2).await;
    assert_eq!(sink.calls.lock()[1], (2, 20, false));
    assert_eq!(request.finalize(20).await, QuotaFinalStatus::Charged);
}

#[tokio::test]
async fn provider_output_total_is_monotonic() {
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
    accumulator.observe_output_total(20);
    assert_eq!(accumulator.observed_output_for_test(), 20);
    sink.wait_for_calls(1).await;
    accumulator.observe_output_total(12);
    assert_eq!(accumulator.observed_output_for_test(), 20);
    tokio::task::yield_now().await;
    assert_eq!(sink.calls.lock().as_slice(), &[(1, 20, false)]);

    accumulator.observe_output_total(35);
    assert_eq!(accumulator.observed_output_for_test(), 35);
    sink.wait_for_calls(2).await;
    assert_eq!(sink.calls.lock()[1], (2, 35, false));
    assert_eq!(request.finalize(35).await, QuotaFinalStatus::Charged);
}

#[tokio::test(start_paused = true)]
async fn hanging_finalization_times_out_and_aborts_the_worker() {
    let sink = Arc::new(HangingSink::new());
    let sink_trait: Arc<dyn QuotaSink> = sink.clone();
    let mut request = AiQuotaRequest::start(
        sink_trait,
        1,
        Duration::from_secs(60),
        Duration::from_secs(60),
        Duration::from_millis(10),
    );
    assert_eq!(request.finalize(1).await, QuotaFinalStatus::TimedOut);
    tokio::task::yield_now().await;
    assert_eq!(sink.calls.load(Ordering::Acquire), 1);
    assert_eq!(sink.cancelled.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn dropping_a_request_aborts_its_hanging_worker() {
    let sink = Arc::new(HangingSink::new());
    let sink_trait: Arc<dyn QuotaSink> = sink.clone();
    let request = AiQuotaRequest::start(
        sink_trait,
        1,
        Duration::from_secs(60),
        Duration::from_secs(60),
        Duration::from_secs(1),
    );
    request.accumulator().add_output_units(1);
    sink.wait_for_call().await;
    drop(request);
    tokio::task::yield_now().await;
    assert_eq!(sink.cancelled.load(Ordering::Acquire), 1);
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

#[tokio::test(start_paused = true)]
async fn final_reconciliation_retries_within_one_total_deadline() {
    let sink = Arc::new(RecordingSink::new());
    sink.fail_next_calls(1);
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

#[tokio::test(start_paused = true)]
async fn final_reconciliation_reports_failure_after_retry_budget() {
    let sink = Arc::new(RecordingSink::new());
    sink.fail_next_calls(u64::MAX);
    let sink_trait: Arc<dyn QuotaSink> = sink.clone();
    let mut request = AiQuotaRequest::start(
        sink_trait,
        10,
        Duration::from_secs(60),
        Duration::from_secs(60),
        Duration::from_millis(35),
    );

    assert_eq!(request.finalize(17).await, QuotaFinalStatus::Failed);
    assert_eq!(
        sink.calls.lock().as_slice(),
        &[(1, 17, true), (2, 17, true), (3, 17, true), (4, 17, true)]
    );
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
