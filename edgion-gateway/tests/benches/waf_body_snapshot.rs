//! Sealed BodyStorage inspection benchmark; no Gateway route is enabled.
//!
//! Run each mode separately (Cargo tracks this compile-time environment input):
//! EDGION_WAF_SNAPSHOT_MODE=timing cargo bench -p edgion-gateway --bench waf_body_snapshot
//! EDGION_WAF_SNAPSHOT_MODE=allocations cargo bench -p edgion-gateway --bench waf_body_snapshot
//! Add EDGION_WAF_SNAPSHOT_SMOKE=1 for two samples per combination.
//! EDGION_WAF_SNAPSHOT_QUERIES=1 selects additional query-focused fixtures instead
//! of the unchanged default body fixtures. Request fixture assembly is untimed.
//!
//! Allocation mode follows the System-wrapper precedent in radix_hostname. It
//! reports successful allocation traffic and live requested bytes across all
//! threads, including storage workers. This is NOT RSS or allocator resident
//! memory: allocator overhead, realloc transients and filesystem page cache are
//! excluded. Timing mode compiles out counter updates. Do not compare timings
//! between the two modes. Files were just written, so reads represent disk-backed
//! storage with warm page cache, not cold-device throughput. Runtime bookkeeping
//! is included; this isolated process runs no unrelated tasks. Awaited blocking
//! jobs may finish Tokio bookkeeping just across a stage boundary, so small
//! baseline-relative heap deltas include that noise, not exact per-request ownership.
//!
//! Capture and inspection are separate. Fixture creation is outside capture,
//! and its backing is dropped before inspection. Generation/runtime setup is
//! excluded. Inspection includes transaction setup and retains the transaction
//! through the measurement endpoint; transaction/view cleanup follows outside
//! that stage. The capture owner is dropped before inspection; only the shared
//! view remains. Post-transaction live delta retains that view and its cache. Each lane/sample creates fresh storage, preventing read_all's
//! cached materialization from contaminating subsequent measurements. Explicit
//! execution caps below are benchmark envelopes, not deployment defaults.

use std::alloc::{GlobalAlloc, Layout, System};
use std::borrow::Cow;
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use bytes::Bytes;
use edgion_gateway::request_body::{BodyStorage, BodyStorageView};
use edgion_waf::seclang::*;
use pingora_core::protocols::http::RequestBodyBuffer;

const MODE: &str = match option_env!("EDGION_WAF_SNAPSHOT_MODE") {
    Some(mode) => mode,
    None => "timing",
};
const ALLOCATIONS: bool = matches!(MODE.as_bytes(), [b'a', ..]);
const CHUNK_BYTES: usize = 64 * 1024;
static CALLS: AtomicUsize = AtomicUsize::new(0);
static REQUESTED: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct CountingSystem;
#[global_allocator]
static ALLOCATOR: CountingSystem = CountingSystem;

fn allocated(bytes: usize) {
    CALLS.fetch_add(1, Ordering::Relaxed);
    REQUESTED.fetch_add(bytes, Ordering::Relaxed);
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

// SAFETY: pointer/layout contracts are forwarded unchanged to System. The
// counters allocate no memory and never inspect the allocation's contents.
unsafe impl GlobalAlloc for CountingSystem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplies GlobalAlloc's allocation contract.
        let pointer = unsafe { System.alloc(layout) };
        if ALLOCATIONS && !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplies GlobalAlloc's allocation contract.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if ALLOCATIONS && !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: pointer/layout belong to this same forwarding allocator.
        unsafe { System.dealloc(pointer, layout) };
        if ALLOCATIONS {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: original pointer/layout and new size obey GlobalAlloc's contract.
        let replacement = unsafe { System.realloc(pointer, layout, size) };
        if ALLOCATIONS && !replacement.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            allocated(size);
        }
        replacement
    }
}

struct Stage {
    started: Instant,
    calls: usize,
    requested: usize,
    live: usize,
}
struct Measurement {
    ns: u128,
    calls: usize,
    requested: usize,
    live_delta: i128,
    peak_extra: usize,
    baseline_live: usize,
    post_transaction_live_delta: Option<i128>,
    post_view_live_delta: Option<i128>,
}
impl Stage {
    fn start() -> Self {
        let live = LIVE.load(Ordering::Relaxed);
        if ALLOCATIONS {
            // LIVE and cumulative traffic are never reset, including cleanup.
            PEAK.store(live, Ordering::Relaxed);
        }
        Self {
            started: Instant::now(),
            calls: CALLS.load(Ordering::Relaxed),
            requested: REQUESTED.load(Ordering::Relaxed),
            live,
        }
    }
    fn finish(self) -> Measurement {
        Measurement {
            ns: Instant::now().saturating_duration_since(self.started).as_nanos(),
            calls: CALLS.load(Ordering::Relaxed) - self.calls,
            requested: REQUESTED.load(Ordering::Relaxed) - self.requested,
            live_delta: LIVE.load(Ordering::Relaxed) as i128 - self.live as i128,
            peak_extra: PEAK.load(Ordering::Relaxed).saturating_sub(self.live),
            baseline_live: self.live,
            post_transaction_live_delta: None,
            post_view_live_delta: None,
        }
    }
}

struct NoIncludes;
impl IncludeSource for NoIncludes {
    fn load(&mut self, _: IncludeRequest<'_>, _: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        Err(IncludeFailure::NotFound)
    }
}

fn generation(subscribers: usize, sparse_queries: bool) -> CompiledGeneration {
    let source = (1..=subscribers)
        .map(|id| {
            let target = if sparse_queries { "ARGS:q0511" } else { "ARGS" };
            format!("SecRule {target} \"@pm hit\" \"id:{id},phase:2,pass,t:lowercase\"\n")
        })
        .collect::<String>();
    let defaults = [PhaseDefaults::pass(Phase::RequestBody, true, true)];
    let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults).unwrap();
    compile_with_includes(
        "json-benchmark",
        source.as_bytes(),
        &mut NoIncludes,
        None,
        context,
        FrontendLimits::default(),
    )
    .unwrap()
    .into_parts()
    .0
}

// Benchmark envelopes only: these values are not production recommendations.
fn limits() -> JsonInspectionLimits {
    JsonInspectionLimits::default()
}

#[derive(Debug, PartialEq, Eq)]
struct ResultSnapshot {
    outcome: Result<PhaseOutcome, RuntimeError>,
    next_phase: Option<Phase>,
    body_error: Option<BodyError>,
    // Canonical logical charge, NOT physical memory usage of either executor.
    logical_retained_bytes: usize,
}

fn fixture(name: &str) -> Vec<u8> {
    if matches!(name, "small_plain" | "small_escaped") {
        let value = if name == "small_escaped" {
            "\\u0068\\u0069\\u0074"
        } else {
            "hit"
        };
        return format!(
            "{{{}}}",
            (0..32)
                .map(|i| format!("\"key{i:03}\":\"{value}\""))
                .collect::<Vec<_>>()
                .join(",")
        )
        .into_bytes();
    }
    if name == "sibling_objects" {
        return format!(
            "[{}]",
            (0..128)
                .map(|_| "{\"name\":\"hit\",\"value\":0}")
                .collect::<Vec<_>>()
                .join(",")
        )
        .into_bytes();
    }
    if matches!(name, "query_single" | "query_many" | "query_sparse") {
        return b"{}".to_vec();
    }
    if name == "large_scalar" {
        return format!("{{\"value\":\"{}HIT\"}}", "x".repeat(256 * 1024)).into_bytes();
    }
    if name == "wide_no_hits" {
        return format!(
            "{{{}}}",
            (0..10_000)
                .rev()
                .map(|i| format!("\"k{i:05}\":\"{}\"", "x".repeat(16)))
                .collect::<Vec<_>>()
                .join(",")
        )
        .into_bytes();
    }
    let value = if name == "escaped_hits" {
        "\\u0078".repeat(253) + "\\u0048\\u0049\\u0054"
    } else {
        "x".repeat(253) + "HIT"
    };

    if name == "duplicates_last_miss" {
        return format!(
            "{{{},\"same\":\"miss\"}}",
            (0..512)
                .map(|_| format!("\"same\":\"{value}\""))
                .collect::<Vec<_>>()
                .join(",")
        )
        .into_bytes();
    }
    assert!(matches!(name, "wide_hits" | "escaped_hits" | "malformed_tail"));
    let mut body = format!(
        "{{{}}}",
        (0..256)
            .map(|i| format!("\"k{i:04}\":\"{value}\""))
            .collect::<Vec<_>>()
            .join(",")
    )
    .into_bytes();
    if name == "malformed_tail" {
        body.pop();
    }
    body
}

struct RequestFixture {
    name: &'static str,
    uri: Cow<'static, [u8]>,
    line: Cow<'static, [u8]>,
}
impl RequestFixture {
    fn new(name: &'static str) -> Self {
        let query = match name {
            "query_single" => "q=HIT".to_owned(),
            "query_many" | "query_sparse" => (0..512)
                .map(|index| format!("q{index:04}=HIT"))
                .collect::<Vec<_>>()
                .join("&"),
            _ => {
                return Self {
                    name,
                    uri: Cow::Borrowed(b"/?q=miss"),
                    line: Cow::Borrowed(b"POST /?q=miss HTTP/1.1"),
                }
            }
        };
        let uri = format!("/?{query}");
        Self {
            name,
            line: Cow::Owned(format!("POST {uri} HTTP/1.1").into_bytes()),
            uri: Cow::Owned(uri.into_bytes()),
        }
    }
}

fn transaction<'g>(
    generation: &'g CompiledGeneration,
    mode: RuleEngineMode,
    request: &RequestFixture,
) -> Transaction<'g> {
    let headers = [RequestHeader::new(b"content-type", b"application/json")];
    let head = RequestHead::new(
        &request.line,
        b"POST",
        &request.uri,
        &request.uri,
        b"HTTP/1.1",
        b"192.0.2.1",
        b"snapshot-benchmark",
        &headers,
    );
    let mut tx = Transaction::new_with_policy(
        generation,
        head,
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        mode,
        RuleCheckPolicy::default(),
    )
    .unwrap();
    tx.execute_request_headers().unwrap();
    tx
}

fn snapshot(tx: &Transaction<'_>, outcome: Result<PhaseOutcome, RuntimeError>) -> ResultSnapshot {
    ResultSnapshot {
        outcome,
        next_phase: tx.next_phase(),
        body_error: tx.request_body_error(),
        logical_retained_bytes: tx.retained_bytes(),
    }
}

fn reference(
    generation: &CompiledGeneration,
    body: &[u8],
    mode: RuleEngineMode,
    request: &RequestFixture,
) -> ResultSnapshot {
    let mut tx = transaction(generation, mode, request);
    let outcome = tx
        .publish_request_body(RequestBody::new(
            Bytes::copy_from_slice(body),
            BodyProcessorSelection::Auto,
        ))
        .and_then(|()| tx.execute_request_body());
    snapshot(&tx, outcome)
}

async fn inspect<'g>(
    generation: &'g CompiledGeneration,
    view: &BodyStorageView,
    explicit_limits: bool,
    mode: RuleEngineMode,
    request: &RequestFixture,
) -> (Transaction<'g>, ResultSnapshot) {
    let mut tx = transaction(generation, mode, request);
    let body = view.read_all().await.unwrap();
    assert_eq!(body.len() as u64, view.len().unwrap());
    let input = RequestBody::new(body, BodyProcessorSelection::Auto);
    let outcome = if explicit_limits {
        tx.publish_request_body_with_json_limits(input, limits())
    } else {
        tx.publish_request_body(input)
    }
    .and_then(|()| tx.execute_request_body());
    let result = snapshot(&tx, outcome);
    (tx, result)
}

async fn sample(
    generation: &CompiledGeneration,
    spill: bool,
    explicit_limits: bool,
    mode: RuleEngineMode,
    expected: &ResultSnapshot,
    request: &RequestFixture,
) -> (usize, Measurement, Measurement) {
    let name = request.name;
    let body = Bytes::from(fixture(name));
    let len = body.len();
    // Every fixture exceeds this threshold, forcing the public spill path.
    let memory_limit = if spill { 1 } else { len + 1 };
    assert!(len > memory_limit || !spill);
    let capture = Stage::start();
    let (mut storage, view) = BodyStorage::new(memory_limit, len as u64);
    for start in (0..len).step_by(CHUNK_BYTES) {
        storage
            .write(&body.slice(start..(start + CHUNK_BYTES).min(len)))
            .await
            .unwrap();
    }
    storage.finish().await.unwrap();
    let capture = capture.finish();
    assert_eq!(view.len().unwrap(), len as u64);
    drop(body);
    drop(storage);
    // Public reader introspection proves the forced storage path without a
    // private hook. This reader never reaches the timed inspection stage.
    let probe = view.snapshot_reader().unwrap();
    assert_eq!(probe.memory_bytes_to_isolate(), Some(if spill { 0 } else { len }));
    drop(probe);
    let inspection = Stage::start();
    let (tx, actual) = inspect(generation, &view, explicit_limits, mode, request).await;
    let mut inspection = inspection.finish();
    assert_eq!(
        &actual, expected,
        "{name}, spill={spill}, explicit_limits={explicit_limits}, {mode:?}"
    );
    black_box(&tx);
    drop(tx);
    drop(actual);
    // Keep the view alive: complete disk reads retain their materialized cache,
    // while the transaction's parsed/virtual state has now been released.
    inspection.post_transaction_live_delta =
        Some(LIVE.load(Ordering::Relaxed) as i128 - inspection.baseline_live as i128);
    drop(view);
    // Relative to inspection start, this also subtracts the captured source
    // already alive at that baseline; negative deltas are expected for memory.
    inspection.post_view_live_delta = Some(LIVE.load(Ordering::Relaxed) as i128 - inspection.baseline_live as i128);
    (len, capture, inspection)
}

fn report(prefix: &str, stage: &str, measurement: &Measurement) {
    if ALLOCATIONS {
        let post_transaction = measurement
            .post_transaction_live_delta
            .map(|bytes| bytes.to_string())
            .unwrap_or_default();
        let post_view = measurement
            .post_view_live_delta
            .map(|bytes| bytes.to_string())
            .unwrap_or_default();
        println!(
            "{prefix},{stage},{},{},{},{},{},{post_transaction},{post_view}",
            measurement.ns, measurement.calls, measurement.requested, measurement.live_delta, measurement.peak_extra
        );
    } else {
        // Empty allocation columns mean unmeasured, not zero allocations.
        println!("{prefix},{stage},{},,,,,,", measurement.ns);
    }
}

fn allocator_self_check() {
    let live = LIVE.load(Ordering::Relaxed);
    let stage = Stage::start();
    let owned = black_box(Box::new([7u8; 257]));
    let mut growing = Vec::<u8>::with_capacity(64);
    growing.resize(64, 1);
    black_box(&growing);
    growing.reserve_exact(960);
    let capacity = growing.capacity();
    black_box(&growing);
    let measured = stage.finish();
    if ALLOCATIONS {
        assert_eq!(measured.calls, 3);
        assert_eq!(measured.requested, 257 + 64 + capacity);
        assert_eq!(measured.live_delta, (257 + capacity) as i128);
        assert_eq!(measured.peak_extra, 257 + capacity);
    } else {
        assert_eq!(measured.calls, 0);
        assert_eq!(measured.requested, 0);
        assert_eq!(measured.live_delta, 0);
        assert_eq!(measured.peak_extra, 0);
    }
    drop(growing);
    drop(owned);
    assert_eq!(LIVE.load(Ordering::Relaxed), live);
}

async fn verify_edited_snapshot(generation: &CompiledGeneration) {
    let request = RequestFixture::new("wide_hits");
    for memory_limit in [1, 1024] {
        for explicit_limits in [false, true] {
            let (mut storage, view) = BodyStorage::new(memory_limit, 1024);
            storage.write(&Bytes::from_static(br#"{"x":"miss"}"#)).await.unwrap();
            storage.finish().await.unwrap();
            drop(storage);
            // Retain original source segments around an inline splice. A full
            // overwrite would turn the disk fixture into a memory-only snapshot.
            view.replace_spans(view.generation(), &[(6..10, Bytes::from_static(b"HIT"))])
                .unwrap();
            let probe = view.snapshot_reader().unwrap();
            assert_eq!(
                probe.memory_bytes_to_isolate(),
                Some(if memory_limit == 1 { 3 } else { 11 })
            );
            drop(probe);
            let expected = reference(generation, br#"{"x":"HIT"}"#, RuleEngineMode::On, &request);
            let (tx, actual) = inspect(generation, &view, explicit_limits, RuleEngineMode::On, &request).await;
            assert_eq!(actual, expected);
            drop(tx);
        }
    }
}

fn main() {
    assert!(
        matches!(MODE, "timing" | "allocations"),
        "invalid compile-time EDGION_WAF_SNAPSHOT_MODE"
    );
    allocator_self_check();
    let smoke = std::env::var_os("EDGION_WAF_SNAPSHOT_SMOKE").is_some();
    let samples = if smoke { 2 } else { 11 };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    println!("schema_version,measurement,arch,os,case,source_bytes,subscribers,engine,storage,lane,sample,stage,elapsed_ns,allocation_calls,requested_bytes,live_delta_bytes,peak_extra_requested_bytes,post_transaction_live_delta_bytes,post_view_live_delta_bytes");
    let query_cases = std::env::var_os("EDGION_WAF_SNAPSHOT_QUERIES").is_some();
    let cases: &[&str] = if query_cases {
        &["query_single", "query_many", "query_sparse"]
    } else {
        &[
            "small_plain",
            "small_escaped",
            "sibling_objects",
            "wide_hits",
            "wide_no_hits",
            "escaped_hits",
            "duplicates_last_miss",
            "malformed_tail",
            "large_scalar",
        ]
    };
    for subscribers in [1, 8, 32] {
        for &name in cases {
            let generation = generation(subscribers, name == "query_sparse");
            runtime.block_on(verify_edited_snapshot(&generation));
            let request = RequestFixture::new(name);
            for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
                let expected = reference(&generation, &fixture(name), mode, &request);
                if query_cases {
                    let outcome = expected.outcome.as_ref().expect("query fixture must execute");
                    assert_eq!(outcome.findings().len(), subscribers);
                    assert_eq!(outcome.omitted_findings(), 0);
                }
                if name == "malformed_tail" && mode == RuleEngineMode::On {
                    assert!(matches!(expected.outcome, Err(RuntimeError::Body { .. })));
                } else {
                    assert!(expected.outcome.is_ok(), "fixture must reach execution: {expected:?}");
                }
                if name == "wide_no_hits" {
                    assert!(expected.outcome.as_ref().unwrap().findings().is_empty());
                } else if matches!(name, "wide_hits" | "escaped_hits") {
                    let outcome = expected.outcome.as_ref().unwrap();
                    assert_eq!(outcome.findings().len(), subscribers);
                    assert_eq!(outcome.omitted_findings(), 0);
                }
                if name == "escaped_hits" {
                    assert_eq!(
                        expected.outcome,
                        reference(&generation, &fixture("wide_hits"), mode, &request).outcome
                    );
                }
                for spill in [false, true] {
                    for explicit_limits in [false, true] {
                        // Warm runtime/storage infrastructure, never reuse the measured capture.
                        runtime.block_on(sample(&generation, spill, explicit_limits, mode, &expected, &request));
                        for index in 0..samples {
                            let (len, capture, inspection) = runtime.block_on(sample(
                                &generation,
                                spill,
                                explicit_limits,
                                mode,
                                &expected,
                                &request,
                            ));
                            let prefix = format!(
                                "1,{MODE},{},{},{name},{len},{subscribers},{mode:?},{},{},{index}",
                                std::env::consts::ARCH,
                                std::env::consts::OS,
                                if spill { "spill_warm_cache" } else { "memory" },
                                if explicit_limits {
                                    "explicit_limits"
                                } else {
                                    "default_limits"
                                }
                            );
                            report(&prefix, "capture", &capture);
                            report(&prefix, "inspection", &inspection);
                        }
                    }
                }
            }
        }
    }
}
