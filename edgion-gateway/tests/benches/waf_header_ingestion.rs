//! Compare the former owned-header bridge with the borrowed WAF ingestion path.
//!
//! Smoke run:
//! `EDGION_WAF_HEADER_BENCH_SMOKE=1 cargo bench -p edgion-gateway --bench waf_header_ingestion`

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use edgion_resources::resources::edgion_plugins::{WafConfig, WafMode};
use edgion_waf::seclang::{
    BodyProcessorSelection, CompiledGeneration, RequestBody, RequestHead, RequestHeader, ResponseHead, ResponseHeader,
    RuntimeLimits, Transaction,
};
use http::{HeaderMap, HeaderName, HeaderValue};

struct CountingAllocator;

static TRACK_ALLOCATIONS: AtomicBool = AtomicBool::new(false);
static ALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

// SAFETY: every operation delegates to `System` with the original pointer and
// layout. The additional relaxed atomics only observe successful allocations.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: this forwards the caller-provided layout unchanged.
        let pointer = unsafe { System.alloc(layout) };
        record_allocation(pointer, layout.size());
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: this forwards the caller-provided layout unchanged.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        record_allocation(pointer, layout.size());
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the pointer and layout come from the matching allocation.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: this forwards the original allocation and requested size.
        let new_pointer = unsafe { System.realloc(pointer, layout, new_size) };
        record_allocation(new_pointer, new_size);
        new_pointer
    }
}

fn record_allocation(pointer: *mut u8, bytes: usize) {
    if !pointer.is_null() && TRACK_ALLOCATIONS.load(Ordering::Relaxed) {
        ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy)]
enum Side {
    Request,
    Response,
}

impl Side {
    const fn label(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Response => "response",
        }
    }
}

#[derive(Clone, Copy)]
enum Bridge {
    Materialized,
    Borrowed,
}

impl Bridge {
    const fn label(self) -> &'static str {
        match self {
            Self::Materialized => "materialized",
            Self::Borrowed => "borrowed",
        }
    }
}

struct Case {
    name: &'static str,
    headers: HeaderMap,
    operations_per_sample: usize,
}

#[derive(Clone, Copy)]
struct AllocationSample {
    calls: u64,
    bytes: u64,
    fingerprint: u64,
}

fn main() {
    let samples = if std::env::var_os("EDGION_WAF_HEADER_BENCH_SMOKE").is_some() {
        11
    } else {
        101
    };
    let generation = Arc::new(compile_generation());
    println!(
        "schema_version,case,side,bridge,headers,header_bytes,allocation_calls,allocated_bytes,samples,operations_per_sample,p50_ns,p95_ns,operations_per_second,fingerprint"
    );
    for case in cases() {
        for side in [Side::Request, Side::Response] {
            assert_bridge_content_equivalent(&case.headers, side);
            let materialized = allocation_sample(&generation, &case.headers, side, Bridge::Materialized);
            let borrowed = allocation_sample(&generation, &case.headers, side, Bridge::Borrowed);
            assert_eq!(
                materialized.fingerprint,
                borrowed.fingerprint,
                "borrowed {} ingestion must preserve transaction results",
                side.label()
            );
            let expected_removed = 1_u64.saturating_add(2_u64.saturating_mul(case.headers.len() as u64));
            assert_eq!(
                materialized.calls.saturating_sub(borrowed.calls),
                expected_removed,
                "borrowed {} ingestion must remove the owned pair vector and both byte vectors per header",
                side.label()
            );
            for (bridge, allocation) in [(Bridge::Materialized, materialized), (Bridge::Borrowed, borrowed)] {
                let durations = timing_samples(
                    &generation,
                    &case.headers,
                    side,
                    bridge,
                    samples,
                    case.operations_per_sample,
                );
                let total_seconds = durations.iter().map(Duration::as_secs_f64).sum::<f64>();
                let total_operations = samples.saturating_mul(case.operations_per_sample);
                println!(
                    "1,{},{},{},{},{},{},{},{},{},{},{},{:.2},{}",
                    case.name,
                    side.label(),
                    bridge.label(),
                    case.headers.len(),
                    header_bytes(&case.headers),
                    allocation.calls,
                    allocation.bytes,
                    samples,
                    case.operations_per_sample,
                    percentile_nanos(&durations, 50),
                    percentile_nanos(&durations, 95),
                    total_operations as f64 / total_seconds,
                    allocation.fingerprint,
                );
            }
        }
    }
}

fn compile_generation() -> CompiledGeneration {
    WafConfig {
        rules: concat!(
            "SecRule REQUEST_HEADERS:X-Bench \"@contains absent-marker\" \"id:1,phase:1,pass\"\n",
            "SecRule RESPONSE_HEADERS:X-Bench \"@contains absent-marker\" \"id:2,phase:3,pass\"\n"
        )
        .to_string(),
        mode: WafMode::On,
        priority: 0,
        request_body: Default::default(),
        ..WafConfig::default()
    }
    .compile_generation()
    .expect("static benchmark WAF rules must compile")
}

fn cases() -> [Case; 2] {
    [
        Case {
            name: "representative",
            headers: headers(16, 32),
            operations_per_sample: 64,
        },
        Case {
            name: "header_heavy",
            headers: headers(512, 96),
            operations_per_sample: 2,
        },
    ]
}

fn headers(count: usize, value_bytes: usize) -> HeaderMap {
    let mut headers = HeaderMap::with_capacity(count);
    for index in 0..count {
        let name = if index == 0 {
            HeaderName::from_static("x-bench")
        } else {
            HeaderName::from_bytes(format!("x-bench-{index:04}").as_bytes())
                .expect("generated benchmark header names must be valid")
        };
        let mut value = vec![b'a' + (index % 20) as u8; value_bytes];
        value[..8].copy_from_slice(format!("{index:08}").as_bytes());
        if index == 0 {
            value[..13].copy_from_slice(b"absent-marker");
        }
        headers.append(
            name,
            HeaderValue::from_bytes(&value).expect("generated benchmark header values must be valid"),
        );
    }
    headers
}

fn assert_bridge_content_equivalent(headers: &HeaderMap, side: Side) {
    match side {
        Side::Request => with_materialized_request_headers(headers, |materialized| {
            with_borrowed_request_headers(headers, |borrowed| {
                assert!(request_header_sequences_equal(materialized, borrowed));

                let mut altered = borrowed.to_vec();
                let first = altered[0];
                let mut altered_value = first.value().to_vec();
                altered_value[0] ^= 1;
                altered[0] = RequestHeader::new(first.name(), &altered_value);
                assert!(
                    !request_header_sequences_equal(materialized, &altered),
                    "request content comparison must reject an equal-length value change"
                );
            });
        }),
        Side::Response => with_materialized_response_headers(headers, |materialized| {
            with_borrowed_response_headers(headers, |borrowed| {
                assert!(response_header_sequences_equal(materialized, borrowed));

                let mut altered = borrowed.to_vec();
                let first = altered[0];
                let mut altered_value = first.value().to_vec();
                altered_value[0] ^= 1;
                altered[0] = ResponseHeader::new(first.name(), &altered_value);
                assert!(
                    !response_header_sequences_equal(materialized, &altered),
                    "response content comparison must reject an equal-length value change"
                );
            });
        }),
    }
}

fn with_materialized_request_headers<T>(headers: &HeaderMap, consume: impl FnOnce(&[RequestHeader<'_>]) -> T) -> T {
    let owned: Vec<_> = headers
        .iter()
        .map(|(name, value)| (name.as_str().as_bytes().to_vec(), value.as_bytes().to_vec()))
        .collect();
    let views: Vec<_> = owned
        .iter()
        .map(|(name, value)| RequestHeader::new(name, value))
        .collect();
    consume(&views)
}

fn with_borrowed_request_headers<T>(headers: &HeaderMap, consume: impl FnOnce(&[RequestHeader<'_>]) -> T) -> T {
    let views: Vec<_> = headers
        .iter()
        .map(|(name, value)| RequestHeader::new(name.as_str().as_bytes(), value.as_bytes()))
        .collect();
    consume(&views)
}

fn with_materialized_response_headers<T>(headers: &HeaderMap, consume: impl FnOnce(&[ResponseHeader<'_>]) -> T) -> T {
    let owned: Vec<_> = headers
        .iter()
        .map(|(name, value)| (name.as_str().as_bytes().to_vec(), value.as_bytes().to_vec()))
        .collect();
    let views: Vec<_> = owned
        .iter()
        .map(|(name, value)| ResponseHeader::new(name, value))
        .collect();
    consume(&views)
}

fn with_borrowed_response_headers<T>(headers: &HeaderMap, consume: impl FnOnce(&[ResponseHeader<'_>]) -> T) -> T {
    let views: Vec<_> = headers
        .iter()
        .map(|(name, value)| ResponseHeader::new(name.as_str().as_bytes(), value.as_bytes()))
        .collect();
    consume(&views)
}

fn request_header_sequences_equal(left: &[RequestHeader<'_>], right: &[RequestHeader<'_>]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.name() == right.name() && left.value() == right.value())
}

fn response_header_sequences_equal(left: &[ResponseHeader<'_>], right: &[ResponseHeader<'_>]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.name() == right.name() && left.value() == right.value())
}

fn allocation_sample(
    generation: &Arc<CompiledGeneration>,
    headers: &HeaderMap,
    side: Side,
    bridge: Bridge,
) -> AllocationSample {
    ALLOCATION_CALLS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    TRACK_ALLOCATIONS.store(true, Ordering::SeqCst);
    let fingerprint = ingest(generation, headers, side, bridge);
    TRACK_ALLOCATIONS.store(false, Ordering::SeqCst);
    AllocationSample {
        calls: ALLOCATION_CALLS.load(Ordering::Relaxed),
        bytes: ALLOCATED_BYTES.load(Ordering::Relaxed),
        fingerprint,
    }
}

fn timing_samples(
    generation: &Arc<CompiledGeneration>,
    headers: &HeaderMap,
    side: Side,
    bridge: Bridge,
    sample_count: usize,
    operations_per_sample: usize,
) -> Vec<Duration> {
    TRACK_ALLOCATIONS.store(false, Ordering::SeqCst);
    for _ in 0..31 {
        black_box(ingest(generation, headers, side, bridge));
    }
    let mut durations = Vec::with_capacity(sample_count);
    for _ in 0..sample_count {
        let started = Instant::now();
        for _ in 0..operations_per_sample {
            black_box(ingest(generation, headers, side, bridge));
        }
        durations.push(Instant::now().saturating_duration_since(started));
    }
    durations.sort_unstable();
    durations
}

fn ingest(generation: &Arc<CompiledGeneration>, headers: &HeaderMap, side: Side, bridge: Bridge) -> u64 {
    match (side, bridge) {
        (Side::Request, Bridge::Materialized) => ingest_materialized_request(generation, headers),
        (Side::Request, Bridge::Borrowed) => ingest_borrowed_request(generation, headers),
        (Side::Response, Bridge::Materialized) => ingest_materialized_response(generation, headers),
        (Side::Response, Bridge::Borrowed) => ingest_borrowed_response(generation, headers),
    }
}

fn ingest_materialized_request(generation: &Arc<CompiledGeneration>, headers: &HeaderMap) -> u64 {
    with_materialized_request_headers(headers, |views| execute_request(generation, views))
}

fn ingest_borrowed_request(generation: &Arc<CompiledGeneration>, headers: &HeaderMap) -> u64 {
    with_borrowed_request_headers(headers, |views| execute_request(generation, views))
}

fn execute_request(generation: &Arc<CompiledGeneration>, headers: &[RequestHeader<'_>]) -> u64 {
    let mut transaction = transaction(generation, headers);
    let outcome = transaction
        .execute_request_headers()
        .expect("benchmark request headers must execute");
    (transaction.retained_bytes() as u64) ^ outcome.findings().len() as u64
}

fn ingest_materialized_response(generation: &Arc<CompiledGeneration>, headers: &HeaderMap) -> u64 {
    with_materialized_response_headers(headers, |views| execute_response(generation, views))
}

fn ingest_borrowed_response(generation: &Arc<CompiledGeneration>, headers: &HeaderMap) -> u64 {
    with_borrowed_response_headers(headers, |views| execute_response(generation, views))
}

fn execute_response(generation: &Arc<CompiledGeneration>, headers: &[ResponseHeader<'_>]) -> u64 {
    let mut transaction = transaction(generation, &[]);
    transaction
        .execute_request_headers()
        .expect("benchmark request phase must advance");
    transaction
        .publish_request_body(RequestBody::new(bytes::Bytes::new(), BodyProcessorSelection::Auto))
        .expect("benchmark empty request body must publish");
    transaction
        .execute_request_body()
        .expect("benchmark request body phase must advance");
    transaction
        .publish_response_head(ResponseHead::new(200, headers))
        .expect("benchmark response headers must publish");
    let outcome = transaction
        .execute_response_headers()
        .expect("benchmark response headers must execute");
    (transaction.retained_bytes() as u64) ^ outcome.findings().len() as u64
}

fn transaction(generation: &Arc<CompiledGeneration>, headers: &[RequestHeader<'_>]) -> Transaction<'static> {
    Transaction::new_shared(
        Arc::clone(generation),
        RequestHead::new(
            b"GET /benchmark HTTP/1.1",
            b"GET",
            b"/benchmark",
            b"/benchmark",
            b"HTTP/1.1",
            b"192.0.2.1",
            b"benchmark-request",
            headers,
        ),
        RuntimeLimits::default(),
    )
    .expect("bounded benchmark transaction must construct")
}

fn header_bytes(headers: &HeaderMap) -> usize {
    headers
        .iter()
        .map(|(name, value)| name.as_str().len().saturating_add(value.as_bytes().len()))
        .sum()
}

fn percentile_nanos(samples: &[Duration], percentile: usize) -> u128 {
    samples[(samples.len() - 1) * percentile / 100].as_nanos()
}
