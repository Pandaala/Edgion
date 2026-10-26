//! Baseline/candidate query-owner microbenchmarks; no transport gain is implied.
//!
//! `cargo bench -p edgion-gateway --bench request_query`
//! `EDGION_QUERY_BENCH_SMOKE=1` reduces repetitions for a quick feasibility run.
//! Set `EDGION_QUERY_ALLOCATIONS=1` while compiling for allocation-only output;
//! leave it unset while compiling for timing with allocation counters removed.

#![allow(dead_code)]

use std::hint::black_box;
use std::time::{Duration, Instant};

// Cargo checks harness-free benches with cfg(test); mounted unit-test bodies
// execute in the library target, while their imports are unused in this binary.
#[allow(unused_imports)]
#[path = "../../src/request_query/mod.rs"]
mod request_query;
use request_query::RequestQuery;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

// Compile-time mode keeps all counters out of timing runs. Allocation runs are
// single-threaded and account for every successful System allocation, including
// fixture backing created before a measured stage. A stage subtracts that live
// baseline; setup, reporting and teardown remain outside it. Requested-byte
// peaks exclude allocator overhead, RSS and allocator-internal realloc scratch.
const ALLOCATIONS: bool = option_env!("EDGION_QUERY_ALLOCATIONS").is_some();
static CALLS: AtomicUsize = AtomicUsize::new(0);
static REQUESTED: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
struct CountingSystem;
#[global_allocator]
static ALLOCATOR: CountingSystem = CountingSystem;

fn allocated(size: usize) {
    CALLS.fetch_add(1, Ordering::Relaxed);
    REQUESTED.fetch_add(size, Ordering::Relaxed);
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

// SAFETY: System receives the original pointer/layout contracts unchanged.
// Atomic bookkeeping neither allocates nor accesses the allocation contents.
unsafe impl GlobalAlloc for CountingSystem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded GlobalAlloc caller contract.
        let pointer = unsafe { System.alloc(layout) };
        if ALLOCATIONS && !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded GlobalAlloc caller contract.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if ALLOCATIONS && !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: pointer/layout came from this forwarding System allocator.
        unsafe { System.dealloc(pointer, layout) };
        if ALLOCATIONS {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: unchanged caller-provided allocation/reallocation contract.
        let replacement = unsafe { System.realloc(pointer, layout, size) };
        if ALLOCATIONS && !replacement.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            allocated(size);
        }
        replacement
    }
}

mod test_allocation_counter {
    use super::*;
    static MEASURING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    pub struct Stats {
        pub calls: usize,
        pub requested_bytes: usize,
        pub peak_extra: usize,
        pub retained_delta: i128,
    }
    pub fn count_allocation_stats<R>(f: impl FnOnce() -> R) -> (R, Stats) {
        assert!(
            !MEASURING.swap(true, Ordering::Relaxed),
            "nested allocation measurements are unsupported"
        );
        struct Guard;
        impl Drop for Guard {
            fn drop(&mut self) {
                MEASURING.store(false, Ordering::Relaxed);
            }
        }
        let _guard = Guard;
        let calls = CALLS.load(Ordering::Relaxed);
        let requested = REQUESTED.load(Ordering::Relaxed);
        let live = LIVE.load(Ordering::Relaxed);
        PEAK.store(live, Ordering::Relaxed);
        let result = f();
        (
            result,
            Stats {
                calls: CALLS.load(Ordering::Relaxed) - calls,
                requested_bytes: REQUESTED.load(Ordering::Relaxed) - requested,
                peak_extra: PEAK.load(Ordering::Relaxed).saturating_sub(live),
                retained_delta: LIVE.load(Ordering::Relaxed) as i128 - live as i128,
            },
        )
    }
}

fn check_allocation_accounting() {
    if !ALLOCATIONS {
        return;
    }
    let backing = vec![0_u8; 32];
    let (_, stats) = test_allocation_counter::count_allocation_stats(|| {
        let mut temporary = Vec::<u8>::with_capacity(8);
        temporary.extend_from_slice(&[0; 8]);
        temporary.reserve_exact(16);
        black_box(&temporary);
        drop(temporary);
    });
    assert_eq!(stats.calls, 2);
    assert_eq!(stats.requested_bytes, 32);
    assert_eq!(stats.peak_extra, 24);
    assert_eq!(stats.retained_delta, 0);
    let (_, stats) = test_allocation_counter::count_allocation_stats(|| drop(backing));
    assert_eq!(stats.retained_delta, -32);
    assert_eq!(stats.peak_extra, 0);
    let (retained, stats) = test_allocation_counter::count_allocation_stats(|| vec![0_u8; 19]);
    assert_eq!(stats.peak_extra, 19);
    assert_eq!(stats.retained_delta, 19);
    drop(retained);
}

fn baseline_hash_arg<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (raw_name, value) = pair.split_once('=')?;
        let matched = raw_name == name
            || (raw_name.contains(['%', '+']) && edgion_resources::uri_utils::url_decode(raw_name) == name);
        matched.then_some(value)
    })
}

fn owner(raw: String) -> RequestQuery {
    let mut query = RequestQuery::default();
    query.capture(Some(raw)).unwrap();
    query
}

fn measure_baseline(raw: &str, name: &str, repetitions: usize, width: usize) -> Duration {
    let mut elapsed = Duration::ZERO;
    for _ in 0..repetitions {
        let fixtures: Vec<_> = (0..width).map(|_| raw.to_owned()).collect();
        let start = Instant::now();
        for fixture in &fixtures {
            black_box(baseline_hash_arg(black_box(fixture), black_box(name)));
        }
        elapsed += start.elapsed();
        black_box(fixtures);
    }
    elapsed
}

fn measure_candidate(raw: &str, name: &str, repetitions: usize, width: usize) -> Duration {
    let mut elapsed = Duration::ZERO;
    for _ in 0..repetitions {
        let fixtures: Vec<_> = (0..width).map(|_| owner(raw.to_owned())).collect();
        let start = Instant::now();
        for fixture in &fixtures {
            black_box(fixture.current_hash_arg_raw(black_box(name)));
        }
        elapsed += start.elapsed();
        black_box(fixtures);
    }
    elapsed
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    (values[values.len() / 2 - 1] + values[values.len() / 2]) / 2.0
}

fn report_hash_case(label: &str, raw: &str, name: &str, repetitions: usize, width: usize) {
    let query = owner(raw.to_owned());
    let (_, baseline_alloc) = test_allocation_counter::count_allocation_stats(|| {
        black_box(baseline_hash_arg(black_box(raw), black_box(name)));
    });
    let (_, first_alloc) = test_allocation_counter::count_allocation_stats(|| {
        black_box(query.current_hash_arg_raw(black_box(name)));
    });
    let (_, repeat_alloc) = test_allocation_counter::count_allocation_stats(|| {
        black_box(query.current_hash_arg_raw(black_box(name)));
    });
    if ALLOCATIONS {
        println!("case={label} baseline_calls={} baseline_cumulative={} baseline_peak={} baseline_retained={} first_calls={} first_cumulative={} first_peak={} first_retained={} repeat_calls={} repeat_peak={} repeat_retained={}",
            baseline_alloc.calls, baseline_alloc.requested_bytes, baseline_alloc.peak_extra, baseline_alloc.retained_delta,
            first_alloc.calls, first_alloc.requested_bytes, first_alloc.peak_extra, first_alloc.retained_delta,
            repeat_alloc.calls, repeat_alloc.peak_extra, repeat_alloc.retained_delta);
        return;
    }
    black_box(measure_baseline(raw, name, 2, width));
    black_box(measure_candidate(raw, name, 2, width));
    let mut base = Vec::new();
    let mut candidate = Vec::new();
    for batch in 0..10 {
        let (b, c) = if batch % 2 == 0 {
            let b = measure_baseline(raw, name, repetitions, width);
            (b, measure_candidate(raw, name, repetitions, width))
        } else {
            let c = measure_candidate(raw, name, repetitions, width);
            (measure_baseline(raw, name, repetitions, width), c)
        };
        let operations = (repetitions * width) as f64;
        base.push(b.as_nanos() as f64 / operations);
        candidate.push(c.as_nanos() as f64 / operations);
    }
    let base_median = median(&mut base);
    let candidate_median = median(&mut candidate);
    let base_mad = median(&mut base.iter().map(|v| (v - base_median).abs()).collect::<Vec<_>>());
    let candidate_mad = median(
        &mut candidate
            .iter()
            .map(|v| (v - candidate_median).abs())
            .collect::<Vec<_>>(),
    );
    println!(
        "case={label} baseline_ns={base_median:.2} candidate_ns={candidate_median:.2} ratio={:.3} baseline_mad_pct={:.2} candidate_mad_pct={:.2}",
        candidate_median / base_median,
        100.0 * base_mad / base_median,
        100.0 * candidate_mad / candidate_median,
    );
}

// Preserve the original boxed cache layout and allocation cost for comparison.
#[allow(clippy::box_collection)]
struct BaselineEntry {
    raw: Option<String>,
    parsed: std::sync::OnceLock<Option<Box<std::collections::HashMap<String, String>>>>,
}

impl BaselineEntry {
    fn new(raw: &str) -> Self {
        Self {
            raw: Some(raw.to_owned()),
            parsed: std::sync::OnceLock::new(),
        }
    }

    fn lookup(&self, name: &str) -> Option<&str> {
        static EMPTY: std::sync::LazyLock<std::collections::HashMap<String, String>> =
            std::sync::LazyLock::new(std::collections::HashMap::new);
        self.parsed
            .get_or_init(|| {
                self.raw
                    .as_deref()
                    .map(edgion_resources::uri_utils::parse_query_string)
                    .filter(|params| !params.is_empty())
                    .map(Box::new)
            })
            .as_deref()
            .unwrap_or(&EMPTY)
            .get(name)
            .map(String::as_str)
    }
}

fn decoded_baseline(raw: &str, name: &str, repetitions: usize, width: usize, warm: bool) -> Duration {
    let mut elapsed = Duration::ZERO;
    for _ in 0..repetitions {
        let fixtures: Vec<_> = (0..width).map(|_| BaselineEntry::new(raw)).collect();
        if warm {
            for fixture in &fixtures {
                black_box(fixture.lookup(name));
            }
        }
        let start = Instant::now();
        for fixture in &fixtures {
            black_box(fixture.lookup(black_box(name)));
        }
        elapsed += start.elapsed();
        black_box(fixtures);
    }
    elapsed
}

fn decoded_candidate(raw: &str, name: &str, repetitions: usize, width: usize, warm: bool) -> Duration {
    let mut elapsed = Duration::ZERO;
    for _ in 0..repetitions {
        let fixtures: Vec<_> = (0..width).map(|_| owner(raw.to_owned())).collect();
        if warm {
            for fixture in &fixtures {
                black_box(fixture.entry_param(name));
            }
        }
        let start = Instant::now();
        for fixture in &fixtures {
            black_box(fixture.entry_param(black_box(name)));
        }
        elapsed += start.elapsed();
        black_box(fixtures);
    }
    elapsed
}

fn report_decoded_case(raw: &str, name: &str, repetitions: usize, width: usize) {
    for warm in [false, true] {
        let old = BaselineEntry::new(raw);
        let new = owner(raw.to_owned());
        if warm {
            black_box(old.lookup(name));
            black_box(new.entry_param(name));
        }
        let (_, old_alloc) = test_allocation_counter::count_allocation_stats(|| {
            black_box(old.lookup(name));
        });
        let (_, new_alloc) = test_allocation_counter::count_allocation_stats(|| {
            black_box(new.entry_param(name));
        });
        if ALLOCATIONS {
            println!("case=decoded_plain_4 warm={warm} baseline_calls={} baseline_cumulative={} baseline_peak={} baseline_retained={} candidate_calls={} candidate_cumulative={} candidate_peak={} candidate_retained={}",
                old_alloc.calls, old_alloc.requested_bytes, old_alloc.peak_extra, old_alloc.retained_delta,
                new_alloc.calls, new_alloc.requested_bytes, new_alloc.peak_extra, new_alloc.retained_delta);
            continue;
        }
        black_box(decoded_baseline(raw, name, 2, width, warm));
        black_box(decoded_candidate(raw, name, 2, width, warm));
        let mut baseline = Vec::new();
        let mut candidate = Vec::new();
        for batch in 0..10 {
            let (b, c) = if batch % 2 == 0 {
                let b = decoded_baseline(raw, name, repetitions, width, warm);
                (b, decoded_candidate(raw, name, repetitions, width, warm))
            } else {
                let c = decoded_candidate(raw, name, repetitions, width, warm);
                (decoded_baseline(raw, name, repetitions, width, warm), c)
            };
            let operations = (repetitions * width) as f64;
            baseline.push(b.as_nanos() as f64 / operations);
            candidate.push(c.as_nanos() as f64 / operations);
        }
        let old_median = median(&mut baseline);
        let new_median = median(&mut candidate);
        let old_mad = median(&mut baseline.iter().map(|v| (v - old_median).abs()).collect::<Vec<_>>());
        let new_mad = median(&mut candidate.iter().map(|v| (v - new_median).abs()).collect::<Vec<_>>());
        println!("case=decoded_plain_4 warm={warm} baseline_ns={old_median:.2} candidate_ns={new_median:.2} ratio={:.3} baseline_mad_pct={:.2} candidate_mad_pct={:.2}",
            new_median / old_median, 100.0 * old_mad / old_median, 100.0 * new_mad / new_median);
    }
}

// Combined Query work, not a network/whole-Gateway throughput benchmark. Both
// fixtures own entry bytes plus a concrete query before timing. Target parsing,
// path/authority validation and transport/header clones are excluded equally.
struct OldSequence {
    entry: BaselineEntry,
    wire: Option<String>,
    detached: Option<String>,
    baseline: Option<String>,
}
struct NewSequence {
    query: RequestQuery,
    wire: Option<String>,
    detached: Option<String>,
    baseline: Option<String>,
}
fn old_remove(wire: &mut Option<String>, name: &str) {
    let Some(raw) = wire.as_deref() else { return };
    let mut removed = false;
    let survivors: Vec<_> = raw
        .split('&')
        .filter(|part| !part.is_empty())
        .filter(|part| {
            let key = part.split_once('=').map_or(*part, |(key, _)| key);
            let matched = if key.contains(['%', '+']) {
                edgion_resources::uri_utils::url_decode(key) == name
            } else {
                key == name
            };
            removed |= matched;
            !matched
        })
        .collect();
    if removed {
        let output = (!survivors.is_empty()).then(|| survivors.join("&"));
        *wire = output;
    }
}
fn run_old_sequence(state: &mut OldSequence) {
    black_box(state.entry.lookup("keep"));
    old_remove(&mut state.wire, "token");
    for _ in 0..8 {
        black_box(baseline_hash_arg(state.wire.as_deref().unwrap_or_default(), "keep"));
    }
    state.detached = state.wire.clone();
    old_remove(&mut state.wire, "secret");
    black_box(state.entry.lookup("token"));
    state.baseline = state.wire.clone();
    for _ in 0..3 {
        state.wire = Some("provider=next".to_owned());
        black_box(&state.wire);
        state.wire = state.baseline.clone();
        black_box(baseline_hash_arg(state.wire.as_deref().unwrap_or_default(), "keep"));
    }
}
fn run_new_sequence(state: &mut NewSequence) {
    black_box(state.query.entry_param("keep"));
    state.query.remove_current_names(&["token"]).unwrap();
    for _ in 0..8 {
        black_box(state.query.current_hash_arg_raw("keep"));
    }
    state.detached = state.query.current_raw().map(str::to_owned);
    state.query.remove_current_names(&["secret"]).unwrap();
    black_box(state.query.entry_param("token"));
    // The real AI baseline includes a concrete header; count its Query copy here.
    state.baseline = state.query.current_raw().map(str::to_owned);
    state.query.capture_baseline().unwrap();
    for _ in 0..3 {
        state.query.replace_raw(Some("provider=next".to_owned())).unwrap();
        state.wire = state.query.current_raw().map(str::to_owned);
        black_box(&state.wire);
        state.query.restore_baseline().unwrap();
        state.wire = state.baseline.clone();
        black_box(state.query.current_hash_arg_raw("keep"));
    }
    state.query.mark_synced();
}
fn old_sequence(raw: &str) -> OldSequence {
    OldSequence {
        entry: BaselineEntry::new(raw),
        wire: Some(raw.to_owned()),
        detached: None,
        baseline: None,
    }
}
fn new_sequence(raw: &str) -> NewSequence {
    NewSequence {
        query: owner(raw.to_owned()),
        wire: Some(raw.to_owned()),
        detached: None,
        baseline: None,
    }
}
fn measure_sequence<T>(raw: &str, repetitions: usize, width: usize, setup: fn(&str) -> T, run: fn(&mut T)) -> Duration {
    let mut elapsed = Duration::ZERO;
    for _ in 0..repetitions {
        let mut fixtures: Vec<_> = (0..width).map(|_| setup(raw)).collect();
        let start = Instant::now();
        for state in &mut fixtures {
            run(black_box(state));
        }
        elapsed += start.elapsed();
        black_box(fixtures);
    }
    elapsed
}
fn report_sequence(label: &str, raw: &str, repetitions: usize, width: usize) {
    let mut old = old_sequence(raw);
    let mut new = new_sequence(raw);
    let (_, b) = test_allocation_counter::count_allocation_stats(|| run_old_sequence(&mut old));
    let (_, c) = test_allocation_counter::count_allocation_stats(|| run_new_sequence(&mut new));
    assert_eq!(old.wire, new.wire);
    assert_eq!(old.detached, new.detached);
    assert_eq!(new.query.current_raw(), old.wire.as_deref());
    if ALLOCATIONS {
        println!("case={label} baseline_calls={} baseline_cumulative={} baseline_peak={} baseline_retained={} candidate_calls={} candidate_cumulative={} candidate_peak={} candidate_retained={}", b.calls,b.requested_bytes,b.peak_extra,b.retained_delta,c.calls,c.requested_bytes,c.peak_extra,c.retained_delta);
        return;
    }
    let mut b = Vec::new();
    let mut c = Vec::new();
    black_box(measure_sequence(raw, 2, width, old_sequence, run_old_sequence));
    black_box(measure_sequence(raw, 2, width, new_sequence, run_new_sequence));
    for batch in 0..10 {
        let mut old_run = || {
            b.push(
                measure_sequence(raw, repetitions, width, old_sequence, run_old_sequence).as_nanos() as f64
                    / (repetitions * width) as f64,
            )
        };
        let mut new_run = || {
            c.push(
                measure_sequence(raw, repetitions, width, new_sequence, run_new_sequence).as_nanos() as f64
                    / (repetitions * width) as f64,
            )
        };
        if batch % 2 == 0 {
            old_run();
            new_run();
        } else {
            new_run();
            old_run();
        }
    }
    let bm = median(&mut b);
    let cm = median(&mut c);
    let bd = median(&mut b.iter().map(|v| (v - bm).abs()).collect::<Vec<_>>());
    let cd = median(&mut c.iter().map(|v| (v - cm).abs()).collect::<Vec<_>>());
    println!("case={label} baseline_ns={bm:.2} candidate_ns={cm:.2} ratio={:.3} baseline_mad_pct={:.2} candidate_mad_pct={:.2}",cm/bm,100.0*bd/bm,100.0*cd/cm);
}

pub(crate) fn run() {
    check_allocation_accounting();
    let smoke = std::env::var_os("EDGION_QUERY_BENCH_SMOKE").is_some();
    let repetitions = if smoke { 8 } else { 128 };
    let width = if smoke { 64 } else { 256 };
    println!(
        "RequestQuery_size={} batches=10 repetitions={repetitions} width={width}",
        std::mem::size_of::<RequestQuery>()
    );
    println!(
        "allocations_enabled={ALLOCATIONS}; bytes=requested_allocator_visible; setup_and_teardown_outside_measurement"
    );
    let small = "first=one&second=two&third=three&last=four";
    report_hash_case("first_plain_4", small, "first", repetitions, width);
    let tail = (0..255)
        .map(|i| format!("tail{i}=abcdefghijklmnopqrstuvwxyz"))
        .collect::<Vec<_>>()
        .join("&");
    let long = format!("first=one&{tail}");
    report_hash_case("first_plain_long_tail", &long, "first", repetitions, width);
    report_hash_case("last_plain_long_tail", &long, "tail254", repetitions, width);
    report_hash_case("missing_plain_long_tail", &long, "absent", repetitions, width);
    report_decoded_case(small, "last", repetitions, width);
    report_sequence(
        "sequence_plain",
        "token=one&keep=value&secret=two&tail=last",
        repetitions,
        width,
    );
    report_sequence(
        "sequence_encoded",
        "to%6ben=one&k%65ep=hello%20world&sec%72et=two&ta%69l=last",
        repetitions,
        width,
    );
}

fn main() {
    run();
}
