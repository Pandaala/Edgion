//! Isolated Cookie planner/cache experiments. No production cache is installed.
#![allow(dead_code)]
use http::{header::COOKIE, HeaderMap, HeaderValue};
use std::hint::black_box;
use std::sync::OnceLock;
use std::time::Instant;
#[allow(unused_imports)]
#[path = "../../src/request_cookie.rs"]
mod request_cookie;
use request_cookie::{lookup, pairs, plan_removal, CookieEditError, CookieEditPlan, FieldScope};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

// Compile-time mode keeps all counters out of timing runs. Allocation runs are
// single-threaded and account for every successful System allocation, including
// fixture backing created before a measured stage. A stage subtracts that live
// baseline; setup, reporting and teardown remain outside it. Requested-byte
// peaks exclude allocator overhead, RSS and allocator-internal realloc scratch.
const ALLOCATIONS: bool = option_env!("EDGION_COOKIE_ALLOCATIONS").is_some();
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

fn original_lookup<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    for header in headers.get_all(COOKIE) {
        let cookies = header.to_str().ok()?;
        if let Some(value) = cookies.split(';').find_map(|pair| {
            let mut parts = pair.trim().splitn(2, '=');
            let key = parts.next()?;
            if key == name {
                parts.next()
            } else {
                None
            }
        }) {
            return Some(value);
        }
    }
    None
}

fn baseline(headers: &HeaderMap, names: &[&str]) -> Option<HeaderValue> {
    let mut survivors = Vec::new();
    for field in headers.get_all(COOKIE) {
        let Ok(text) = std::str::from_utf8(field.as_bytes()) else {
            continue;
        };
        for segment in text.split(';') {
            let raw = segment.trim();
            if !raw.is_empty() && !names.contains(&raw.split('=').next().unwrap_or("")) {
                survivors.push(raw);
            }
        }
    }
    if survivors.is_empty() {
        None
    } else {
        let mut value = HeaderValue::try_from(survivors.join("; ")).unwrap();
        value.set_sensitive(true);
        Some(value)
    }
}

#[derive(Clone, Copy)]
struct Span {
    field: usize,
    name_start: usize,
    name_len: usize,
    value_start: usize,
    value_len: usize,
}
struct Index {
    spans: Vec<Span>,
    unreadable: Option<usize>,
}
#[derive(Default)]
struct Candidate {
    parsed: OnceLock<Option<Box<Index>>>,
}
impl Candidate {
    fn get<'a>(&self, headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
        // Prototype is deliberately scoped to one immutable map per instance.
        // Production owner binding and invalidation are NOT implied here.
        let index = self.parsed.get_or_init(|| {
            const BUDGET: usize = 8192;
            let mut count = 0usize;
            let mut unreadable = None;
            for (field, value) in headers.get_all(COOKIE).iter().enumerate() {
                let Ok(text) = value.to_str() else {
                    unreadable = Some(field);
                    break;
                };
                for pair in pairs(text) {
                    if pair.value.is_some() {
                        count = count.checked_add(1)?;
                        if count
                            .checked_mul(std::mem::size_of::<Span>())?
                            .checked_add(std::mem::size_of::<Index>())?
                            > BUDGET
                        {
                            return None;
                        }
                    }
                }
            }
            let mut spans = Vec::new();
            spans.try_reserve_exact(count).ok()?;
            if spans
                .capacity()
                .checked_mul(std::mem::size_of::<Span>())?
                .checked_add(std::mem::size_of::<Index>())?
                > BUDGET
            {
                return None;
            }
            for (field, value) in headers.get_all(COOKIE).iter().enumerate() {
                let Ok(text) = value.to_str() else { break };
                for pair in pairs(text) {
                    if let Some(value) = pair.value {
                        spans.push(Span {
                            field,
                            name_start: pair.name.as_ptr() as usize - text.as_ptr() as usize,
                            name_len: pair.name.len(),
                            value_start: value.as_ptr() as usize - text.as_ptr() as usize,
                            value_len: value.len(),
                        });
                    }
                }
            }
            Some(Box::new(Index { spans, unreadable }))
        });
        let Some(index) = index else {
            return lookup(headers, name, FieldScope::All);
        };
        let mut fields = headers.get_all(COOKIE).iter();
        let mut field_id = 0;
        let mut field = fields.next()?;
        for span in &index.spans {
            while field_id < span.field {
                field = fields.next()?;
                field_id += 1;
            }
            let bytes = field.as_bytes();
            if bytes.get(span.name_start..span.name_start + span.name_len)? == name.as_bytes() {
                return std::str::from_utf8(bytes.get(span.value_start..span.value_start + span.value_len)?).ok();
            }
        }

        None
    }
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}
fn compare(label: &str, mut old: impl FnMut(), mut new: impl FnMut(), iterations: usize) {
    if ALLOCATIONS {
        return;
    }
    let mut old_runs = Vec::new();
    let mut new_runs = Vec::new();
    for batch in 0..10 {
        let mut run = |first: bool| {
            let start = Instant::now();
            for _ in 0..iterations {
                if first {
                    old()
                } else {
                    new()
                }
            }
            let elapsed = start.elapsed().as_nanos() as f64 / iterations as f64;
            if first {
                old_runs.push(elapsed)
            } else {
                new_runs.push(elapsed)
            }
        };
        run(batch % 2 == 0);
        run(batch % 2 != 0);
    }
    let old_median = median(&mut old_runs);
    let new_median = median(&mut new_runs);
    let mut old_deviation: Vec<_> = old_runs.iter().map(|v| (v - old_median).abs()).collect();
    let mut new_deviation: Vec<_> = new_runs.iter().map(|v| (v - new_median).abs()).collect();
    println!(
        "{label}: baseline={old_median:.1}ns MAD={:.1} candidate={new_median:.1}ns MAD={:.1} delta={:.1}%",
        median(&mut old_deviation),
        median(&mut new_deviation),
        (new_median / old_median - 1.0) * 100.0
    );
}
fn main() {
    if ALLOCATIONS {
        let (held, stats) = test_allocation_counter::count_allocation_stats(|| vec![0u8; black_box(37)]);
        assert_eq!(stats.calls, 1);
        assert_eq!(stats.requested_bytes, 37);
        assert_eq!(stats.peak_extra, 37);
        assert_eq!(stats.retained_delta, 37);
        drop(held);
        let (_, stats) = test_allocation_counter::count_allocation_stats(|| {
            let temporary = vec![0u8; black_box(19)];
            black_box(&temporary);
        });
        assert_eq!(stats.peak_extra, 19);
        assert_eq!(stats.retained_delta, 0);
    }
    let iterations = if std::env::var_os("EDGION_COOKIE_BENCH_SMOKE").is_some() {
        100
    } else {
        10000
    };
    println!(
        "isolated prototype; cache inline={} bytes; allocations={ALLOCATIONS}",
        std::mem::size_of::<Candidate>()
    );
    for count in [1, 4, 32, 256] {
        for long in [false, true] {
            let raw = (0..count)
                .map(|i| format!("k{i}={}", if long { "x".repeat(256) } else { "v".into() }))
                .collect::<Vec<_>>()
                .join("; ");
            let mut headers = HeaderMap::new();
            headers.insert(COOKIE, HeaderValue::from_str(&raw).unwrap());
            for name in ["k0".to_owned(), format!("k{}", count - 1), "missing".into()] {
                let label = format!("pairs={count} long={long} name={name}");
                compare(
                    &format!("delete {label}"),
                    || {
                        black_box(baseline(black_box(&headers), &[&name]));
                    },
                    || {
                        black_box(plan_removal(black_box(&headers), &[&name]).unwrap());
                    },
                    iterations,
                );
                compare(
                    &format!("stateless read {label}"),
                    || {
                        black_box(original_lookup(black_box(&headers), &name));
                    },
                    || {
                        black_box(lookup(black_box(&headers), &name, FieldScope::All));
                    },
                    iterations,
                );
                compare(
                    &format!("cold cache {label}"),
                    || {
                        black_box(lookup(black_box(&headers), &name, FieldScope::All));
                    },
                    || {
                        let cache = Candidate::default();
                        black_box(cache.get(black_box(&headers), &name));
                    },
                    iterations,
                );
                let cache = Candidate::default();
                black_box(cache.get(&headers, &name));
                compare(
                    &format!(
                        "warm cache cached={} {label}",
                        cache.parsed.get().is_some_and(Option::is_some)
                    ),
                    || {
                        black_box(lookup(black_box(&headers), &name, FieldScope::All));
                    },
                    || {
                        black_box(cache.get(black_box(&headers), &name));
                    },
                    iterations,
                );
                if ALLOCATIONS {
                    let (_, stats) = test_allocation_counter::count_allocation_stats(|| baseline(&headers, &[&name]));
                    println!(
                        "baseline delete allocation {label}: calls={} requested={} peak-extra={} retained-delta={}",
                        stats.calls, stats.requested_bytes, stats.peak_extra, stats.retained_delta
                    );
                    let (_, stats) =
                        test_allocation_counter::count_allocation_stats(|| plan_removal(&headers, &[&name]).unwrap());
                    println!(
                        "delete allocation {label}: calls={} requested={} peak-extra={} retained-delta={}",
                        stats.calls, stats.requested_bytes, stats.peak_extra, stats.retained_delta
                    );
                    let (cache, stats) = test_allocation_counter::count_allocation_stats(|| {
                        let cache = Candidate::default();
                        black_box(cache.get(&headers, &name));
                        cache
                    });
                    println!(
                        "cache allocation {label}: calls={} requested={} peak-extra={} retained-delta={}",
                        stats.calls, stats.requested_bytes, stats.peak_extra, stats.retained_delta
                    );
                    drop(cache);
                }
            }
        }
    }
    additional_cases(iterations);
}

// Rejected measured prototype retained solely for reproducibility.
/// Two passes over a repeatable borrowed map, without a survivor vector.
/// The caller publishes only after this complete plan has succeeded.
fn rejected_two_pass_removal(headers: &HeaderMap, names: &[&str]) -> Result<CookieEditPlan, CookieEditError> {
    if names.is_empty() {
        return Ok(CookieEditPlan::NoChange);
    }
    let mut saw_field = false;
    let mut matched = false;
    let mut output_len = 0usize;
    let mut survivors = 0usize;
    for field in headers.get_all(COOKIE) {
        saw_field = true;
        let text = std::str::from_utf8(field.as_bytes()).map_err(|_| CookieEditError::Unsanitizable)?;
        for pair in pairs(text) {
            if names.contains(&pair.name) {
                matched = true;
            } else {
                output_len = output_len
                    .checked_add(pair.raw.len())
                    .and_then(|length| length.checked_add(if survivors == 0 { 0 } else { 2 }))
                    .ok_or(CookieEditError::Internal)?;
                survivors = survivors.checked_add(1).ok_or(CookieEditError::Internal)?;
            }
        }
    }
    if !saw_field {
        return Ok(CookieEditPlan::Absent);
    }
    if !matched {
        return Ok(CookieEditPlan::NoChange);
    }
    if survivors == 0 {
        return Ok(CookieEditPlan::Remove);
    }
    let mut output = String::new();
    output
        .try_reserve_exact(output_len)
        .map_err(|_| CookieEditError::Internal)?;
    for field in headers.get_all(COOKIE) {
        let text = std::str::from_utf8(field.as_bytes()).map_err(|_| CookieEditError::Unsanitizable)?;
        for pair in pairs(text) {
            if !names.contains(&pair.name) {
                if !output.is_empty() {
                    output.push_str("; ");
                }
                output.push_str(pair.raw);
            }
        }
    }
    debug_assert_eq!(output.len(), output_len);
    let mut value = HeaderValue::try_from(output).map_err(|_| CookieEditError::Internal)?;
    value.set_sensitive(true);
    Ok(CookieEditPlan::Replace(value))
}

fn allocation_row<R>(label: &str, operation: impl FnOnce() -> R) {
    if !ALLOCATIONS {
        return;
    }
    let (held, stats) = test_allocation_counter::count_allocation_stats(operation);
    println!(
        "{label}: calls={} requested={} peak-extra={} retained-delta={}",
        stats.calls, stats.requested_bytes, stats.peak_extra, stats.retained_delta
    );
    black_box(&held);
}

fn additional_cases(iterations: usize) {
    let mut fixtures = vec![("absent", HeaderMap::new())];
    for (label, lines) in [
        ("empty", vec![String::new()]),
        ("many-fields", (0..32).map(|i| format!("k{i}=value")).collect()),
        (
            "dense",
            vec!["flag; k0=; k1=one; k0=two;; quoted=\"a=b\"; raw=%2F".into()],
        ),
        ("utf8", vec!["k0=one; label=é; k1=two".into()]),
    ] {
        let mut headers = HeaderMap::new();
        for line in lines {
            headers.append(COOKIE, HeaderValue::from_str(&line).unwrap());
        }
        fixtures.push((label, headers));
    }
    for (label, headers) in &fixtures {
        for names in [
            vec!["missing"],
            vec!["k0", "k1"],
            vec!["k0", "k1", "flag", "quoted", "raw", "label"],
        ] {
            let case = format!("matrix {label} names={}", names.len());
            compare(
                &case,
                || {
                    black_box(baseline(black_box(headers), &names));
                },
                || {
                    black_box(plan_removal(black_box(headers), &names).unwrap());
                },
                iterations,
            );
            allocation_row(&format!("baseline {case}"), || baseline(headers, &names));
            allocation_row(&format!("current {case}"), || plan_removal(headers, &names).unwrap());
        }
        compare(
            &format!("mixed reads {label}"),
            || {
                for name in ["k0", "k1", "missing", "k31"] {
                    black_box(original_lookup(black_box(headers), name));
                }
            },
            || {
                for name in ["k0", "k1", "missing", "k31"] {
                    black_box(lookup(black_box(headers), name, FieldScope::All));
                }
            },
            iterations,
        );
    }
    let mut source = HeaderMap::new();
    source.append(COOKIE, HeaderValue::from_static("token=credential; keep=one"));
    source.append(COOKIE, HeaderValue::from_static("other=two; token=credential"));
    // Prepared HeaderMaps are outside timing. This covers only Cookie helpers,
    // HeaderMap publication and owned exports, not Pingora adapters or network.
    if !ALLOCATIONS {
        let count = if iterations < 1000 { 32 } else { 512 };
        let mut old_runs = Vec::new();
        let mut new_runs = Vec::new();
        for batch in 0..10 {
            for old in [batch % 2 == 0, batch % 2 != 0] {
                let mut prepared: Vec<_> = (0..count).map(|_| source.clone()).collect();
                let start = Instant::now();
                for headers in &mut prepared {
                    cookie_sequence(headers, old);
                }
                let elapsed = start.elapsed().as_nanos() as f64 / count as f64;
                if old {
                    old_runs.push(elapsed);
                } else {
                    new_runs.push(elapsed);
                }
                black_box(&prepared);
            }
        }
        let old = median(&mut old_runs);
        let new = median(&mut new_runs);
        let mut old_mad: Vec<_> = old_runs.iter().map(|v| (v - old).abs()).collect();
        let mut new_mad: Vec<_> = new_runs.iter().map(|v| (v - new).abs()).collect();
        println!(
            "Cookie helper sequence: baseline={old:.1}ns MAD={:.1} candidate={new:.1}ns MAD={:.1} delta={:.1}%",
            median(&mut old_mad),
            median(&mut new_mad),
            (new / old - 1.0) * 100.0
        );
    }
    let mut old_headers = source.clone();
    let mut new_headers = source.clone();
    allocation_row("baseline Cookie helper sequence", || {
        cookie_sequence(&mut old_headers, true)
    });
    allocation_row("current Cookie helper sequence", || {
        cookie_sequence(&mut new_headers, false)
    });
    assert_eq!(old_headers, new_headers);
}

fn cookie_sequence(headers: &mut HeaderMap, old: bool) {
    if old {
        black_box(original_lookup(headers, "token"));
    } else {
        black_box(lookup(headers, "token", FieldScope::All));
    }
    if old {
        match baseline(headers, &["token"]) {
            Some(value) => {
                headers.insert(COOKIE, value);
            }
            None => {
                headers.remove(COOKIE);
            }
        }
    } else {
        match plan_removal(headers, &["token"]).unwrap() {
            CookieEditPlan::Replace(value) => {
                headers.insert(COOKIE, value);
            }
            CookieEditPlan::Remove => {
                headers.remove(COOKIE);
            }
            CookieEditPlan::Absent | CookieEditPlan::NoChange => {}
        }
    }
    for name in ["keep", "other", "token"] {
        if old {
            black_box(original_lookup(headers, name));
        } else {
            black_box(lookup(headers, name, FieldScope::All));
        }
    }
    let webhook = headers.clone();
    headers.append(COOKIE, HeaderValue::from_static("later=value"));
    if old {
        black_box(original_lookup(headers, "later"));
    } else {
        black_box(lookup(headers, "later", FieldScope::All));
    }
    let mirror = headers.clone();
    black_box((&webhook, &mirror));
}
