//! Bounded Route hostname benchmark; does not modify certificate matching.
//!
//! Run: cargo bench -p edgion-gateway --bench route_hostname
//! Smoke: EDGION_HOSTNAME_BENCH_SMOKE=1 cargo bench -p edgion-gateway --bench route_hostname
//!
//! CSV timings are batch means, NOT latency percentiles. Lookup uses repeatedly
//! warmed inputs and includes the production ID-to-content lookup. Cold pattern
//! compilation/group aggregation is excluded from tree-build timing. Allocation
//! figures are requested bytes reported by System, not allocator resident bytes
//! or RSS. The global counting overhead affects allocation-heavy measurements;
//! this executable creates no worker threads and owns its own allocator.

use edgion_gateway::runtime::matching::hostname::{CompiledHostnamePattern, HostContentChange, HostnameMatcher};
use edgion_gateway::runtime::matching::hostname_input::HostnameQuery;
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

struct CountingSystem;

static CALLS: AtomicUsize = AtomicUsize::new(0);
static REQUESTED: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn allocated(bytes: usize) {
    CALLS.fetch_add(1, Ordering::Relaxed);
    REQUESTED.fetch_add(bytes, Ordering::Relaxed);
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

// SAFETY: every operation delegates unchanged pointer/layout contracts to System.
// Accounting uses allocation-free atomics and never dereferences allocated memory.
unsafe impl GlobalAlloc for CountingSystem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplies the GlobalAlloc allocation contract.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplies the GlobalAlloc allocation contract.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: pointer/layout are forwarded to their original allocator.
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: the original allocation and new size obey GlobalAlloc's contract.
        let replacement = unsafe { System.realloc(pointer, layout, size) };
        if !replacement.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            allocated(size);
        }
        replacement
    }
}

#[global_allocator]
static ALLOCATOR: CountingSystem = CountingSystem;

struct Measurement {
    ns: u128,
    calls: usize,
    requested: usize,
    live_delta: i128,
    peak_extra: usize,
    live_total: usize,
    peak_total: usize,
}

fn measured<T>(run: impl FnOnce() -> T) -> (T, Measurement) {
    let live = LIVE.load(Ordering::Relaxed);
    let calls = CALLS.load(Ordering::Relaxed);
    let requested = REQUESTED.load(Ordering::Relaxed);
    PEAK.store(live, Ordering::Relaxed);
    let start = Instant::now();
    let output = run();
    let ns = Instant::now().saturating_duration_since(start).as_nanos();
    let result = Measurement {
        ns,
        calls: CALLS.load(Ordering::Relaxed) - calls,
        requested: REQUESTED.load(Ordering::Relaxed) - requested,
        live_delta: LIVE.load(Ordering::Relaxed) as i128 - live as i128,
        peak_extra: PEAK.load(Ordering::Relaxed).saturating_sub(live),
        live_total: LIVE.load(Ordering::Relaxed),
        peak_total: PEAK.load(Ordering::Relaxed),
    };
    (output, result)
}

fn report(engine: &str, operation: &str, keys: usize, iterations: usize, result: &Measurement) {
    println!(
        "1,{engine},{operation},{keys},{},{},{iterations},{},{},{},{},{},{},{}",
        std::env::consts::ARCH,
        std::env::consts::OS,
        result.ns / iterations as u128,
        result.calls,
        result.requested,
        result.live_delta,
        result.peak_extra,
        result.live_total,
        result.peak_total
    );
}

fn pattern(value: &str) -> CompiledHostnamePattern {
    CompiledHostnamePattern::parse(value).expect("benchmark configured hostnames are valid")
}

type Groups = HashMap<CompiledHostnamePattern, Arc<usize>>;

fn groups(keys: usize) -> Groups {
    let mut result: Groups = [
        "api.sub.example.com",
        "*.example.com",
        "*.sub.example.com",
        "*.abcdefghijklmn.com",
        "grpc",
        "*.com",
    ]
    .into_iter()
    .enumerate()
    .map(|(id, name)| (pattern(name), Arc::new(id)))
    .collect();
    for id in result.len()..keys {
        let name = if id % 2 == 0 {
            format!("host{id}.example.com")
        } else {
            format!("*.host{id}.example.com")
        };
        result.insert(pattern(&name), Arc::new(id));
    }
    result
}

/// Benchmark-only corrected HashHost reference. Exact keys never satisfy a
/// wildcard probe: wildcard storage keys start with a dot, exact keys do not.
/// Its value payload is the same integer used by the production generic index.
struct CorrectedHash {
    values: HashMap<String, usize>,
    wildcard_lengths: Vec<usize>,
}

impl CorrectedHash {
    fn new(groups: &Groups) -> Self {
        let mut values = HashMap::with_capacity(groups.len());
        let mut wildcard_lengths = Vec::new();
        for (pattern, value) in groups {
            let key = pattern.as_str().strip_prefix('*').unwrap_or(pattern.as_str());
            if pattern.is_wildcard() {
                wildcard_lengths.push(key.len());
            }
            values.insert(key.to_string(), **value);
        }
        wildcard_lengths.sort_unstable_by(|a, b| b.cmp(a));
        wildcard_lengths.dedup();
        Self {
            values,
            wildcard_lengths,
        }
    }

    fn first(&self, query: &str) -> Option<usize> {
        if let Some(value) = self.values.get(query) {
            return Some(*value);
        }
        for length in &self.wildcard_lengths {
            if query.len() > *length {
                let suffix = &query[query.len() - length..];
                if suffix.starts_with('.') {
                    if let Some(value) = self.values.get(suffix) {
                        return Some(*value);
                    }
                }
            }
        }
        None
    }

    fn checksum(&self, query: &str) -> usize {
        let mut value = self.values.get(query).map_or(0, |value| value + 1);
        for length in &self.wildcard_lengths {
            if query.len() <= *length {
                continue;
            }
            let suffix = &query[query.len() - length..];
            if suffix.starts_with('.') {
                if let Some(found) = self.values.get(suffix) {
                    value = value.wrapping_mul(31).wrapping_add(found + 1);
                }
            }
        }
        value
    }
}

fn checksum(matcher: &HostnameMatcher<usize>, query: HostnameQuery<'_>) -> usize {
    matcher
        .matching_values(query)
        .fold(0usize, |value, found| value.wrapping_mul(31).wrapping_add(found + 1))
}

fn lookup_measurements(matcher: &HostnameMatcher<usize>, reference: &CorrectedHash, keys: usize, iterations: usize) {
    // Successful protocol matching normally stops at the first hostname group.
    // Report that path separately from the complete continuation stream below.
    for (shape, input) in [
        ("lookup_exact_first", "api.sub.example.com"),
        ("lookup_wildcard_first", "random.example.com"),
        ("lookup_short_first", "grpc"),
    ] {
        let query = HostnameQuery::new(input).unwrap();
        assert_eq!(matcher.matching_values(query).next().copied(), reference.first(input));
        for _ in 0..256 {
            black_box(matcher.matching_values(query).next().copied());
            black_box(reference.first(input));
        }
        let (_, timing) = measured(|| {
            for _ in 0..iterations {
                black_box(matcher.matching_values(black_box(query)).next().copied());
            }
        });
        assert_eq!(timing.calls, 0);
        report("radix", shape, keys, iterations, &timing);
        let (_, timing) = measured(|| {
            for _ in 0..iterations {
                black_box(reference.first(black_box(input)));
            }
        });
        assert_eq!(timing.calls, 0);
        report("corrected_hash", shape, keys, iterations, &timing);
    }
    for (shape, input) in [
        ("lookup_exact", "api.sub.example.com"),
        ("lookup_wildcard", "random.example.com"),
        ("lookup_miss", "outside.invalid"),
        ("lookup_continuation", "foo.api.sub.example.com"),
        ("lookup_short", "grpc"),
    ] {
        let query = HostnameQuery::new(input).unwrap();
        assert_eq!(checksum(matcher, query), reference.checksum(input));
        for _ in 0..256 {
            black_box(checksum(matcher, query));
            black_box(reference.checksum(input));
        }
        let (_, timing) = measured(|| {
            for _ in 0..iterations {
                black_box(checksum(matcher, black_box(query)));
            }
        });
        assert_eq!(timing.calls, 0, "normalized radix lookup allocates");
        report("radix", shape, keys, iterations, &timing);
        let (_, timing) = measured(|| {
            for _ in 0..iterations {
                black_box(reference.checksum(black_box(input)));
            }
        });
        assert_eq!(timing.calls, 0, "corrected hash lookup allocates");
        report("corrected_hash", shape, keys, iterations, &timing);
    }
}

fn size_measurements(keys: usize, iterations: usize) {
    let source = groups(keys);
    let prepared = source.clone();
    let (matcher, build) = measured(|| HostnameMatcher::build(prepared));
    report("radix", "full_build", keys, 1, &build);
    let (reference, build) = measured(|| CorrectedHash::new(&source));
    report("corrected_hash", "full_build", keys, 1, &build);
    lookup_measurements(&matcher, &reference, keys, iterations);

    let changes = [HostContentChange::Upsert(
        pattern("api.sub.example.com"),
        Arc::new(keys + 1),
    )];
    let (patched, timing) = measured(|| matcher.with_changes(changes));
    report("radix", "one_content_patch_old_retained", keys, 1, &timing);
    let query = HostnameQuery::new("api.sub.example.com").unwrap();
    assert_eq!(*matcher.matching_values(query).next().unwrap(), 0);
    assert_eq!(*patched.matching_values(query).next().unwrap(), keys + 1);

    let prepared = source.clone();
    let (reconnected, timing) = measured(|| patched.with_groups(prepared));
    report(
        "radix",
        "full_list_same_keyset_one_changed_old_retained",
        keys,
        1,
        &timing,
    );
    // As with other build measurements, prepare payloads before timing the
    // matcher operation. Every Arc is fresh, even though logical values agree.
    let prepared = source
        .iter()
        .map(|(key, value)| (key.clone(), Arc::new(**value)))
        .collect();
    let (replaced, timing) = measured(|| reconnected.with_groups(prepared));
    report(
        "radix",
        "full_list_same_keyset_all_fresh_old_retained",
        keys,
        1,
        &timing,
    );
    assert_eq!(checksum(&reconnected, query), checksum(&replaced, query));
    let exact_key = pattern("api.sub.example.com");
    assert!(!Arc::ptr_eq(
        reconnected.get(&exact_key).unwrap(),
        replaced.get(&exact_key).unwrap()
    ));
    let added_key = pattern("added.example.com");
    let changes = [HostContentChange::Upsert(added_key.clone(), Arc::new(keys + 2))];
    let (added, timing) = measured(|| reconnected.with_changes(changes));
    report("radix", "add_key_remap_old_retained", keys, 1, &timing);
    assert_eq!(added.len(), keys + 1);
    assert_eq!(checksum(&matcher, query), checksum(&added, query));
    let (removed, timing) = measured(|| added.with_changes([HostContentChange::Remove(added_key)]));
    report("radix", "remove_key_remap_old_retained", keys, 1, &timing);
    assert_eq!(removed.len(), keys);
    assert_eq!(checksum(&removed, query), checksum(&matcher, query));
    let (_, timing) = measured(|| drop((patched, reconnected, replaced, added, removed)));
    report("radix", "drop_retained_generations", keys, 1, &timing);
    drop(reference);
    drop(source);
    let (_, timing) = measured(|| drop(matcher));
    report("radix", "drop_final_reader", keys, 1, &timing);
}

fn deep_measurements(iterations: usize) {
    let query = (0..127).map(|_| "a").collect::<Vec<_>>().join(".");
    assert_eq!(query.len(), 253);
    let mut groups = HashMap::new();
    groups.insert(pattern(&query), Arc::new(127));
    for labels in 1..=126 {
        let suffix = (0..labels).map(|_| "a").collect::<Vec<_>>().join(".");
        groups.insert(pattern(&format!("*.{suffix}")), Arc::new(labels));
    }
    let reference = CorrectedHash::new(&groups);
    let matcher = HostnameMatcher::build(groups);
    let query = HostnameQuery::new(&query).unwrap();
    assert_eq!(matcher.matching_values(query).count(), 127);
    assert_eq!(checksum(&matcher, query), reference.checksum(query.as_str()));
    let (_, timing) = measured(|| {
        for _ in 0..iterations {
            black_box(checksum(&matcher, black_box(query)));
        }
    });
    assert_eq!(timing.calls, 0);
    report("radix", "253_byte_127_hits", 127, iterations, &timing);
    let (_, timing) = measured(|| {
        for _ in 0..iterations {
            black_box(reference.checksum(black_box(query.as_str())));
        }
    });
    assert_eq!(timing.calls, 0);
    report("corrected_hash", "253_byte_127_hits", 127, iterations, &timing);
}

fn main() {
    let smoke = std::env::var_os("EDGION_HOSTNAME_BENCH_SMOKE").is_some();
    let iterations = if smoke { 1_000 } else { 100_000 };
    println!("schema_version,engine,operation,keys,arch,os,iterations,mean_ns,allocation_calls,requested_bytes,live_delta_bytes,peak_extra_bytes,live_total_bytes,peak_total_bytes");
    // Initialize output machinery before exact live-byte lifecycle assertions.
    for keys in [10, 1_000, 100_000] {
        let baseline = LIVE.load(Ordering::Relaxed);
        size_measurements(keys, iterations);
        assert_eq!(
            LIVE.load(Ordering::Relaxed),
            baseline,
            "matcher lifecycle leaked accounted bytes"
        );
    }
    deep_measurements(if smoke { 100 } else { 10_000 });
    let prepared: Vec<_> = (0..1_000).map(|_| groups(100)).collect();
    let (listeners, timing) = measured(|| prepared.into_iter().map(HostnameMatcher::build).collect::<Vec<_>>());
    report("radix", "1000_listeners_x_100_keys_build", 100_000, 1, &timing);
    let (_, timing) = measured(|| drop(listeners));
    report("radix", "1000_listeners_x_100_keys_drop", 100_000, 1, &timing);
}
