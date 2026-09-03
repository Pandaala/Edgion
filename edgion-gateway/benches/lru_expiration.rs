//! Capacity-scaled insertion benchmark for the LRU cache expiration index.
//!
//! Guards the objective of the bounded-expiration refactor: full-cache
//! insertion latency must be independent of cache capacity. Before the refactor
//! a full-cache insert walked every live node to discover whether anything had
//! expired, so p50/p99 grew linearly with capacity. The expiration index answers
//! the same question from its first record, so the `live` workload must stay
//! flat across 1K, 16K, and 256K.
//!
//! Percentiles are taken over INDIVIDUAL insert timings, not over per-sample
//! means. Timing each insert costs two `Instant::now()` calls (~25 ns on the
//! reference host, ~1.5% of a measured insert), and that bias is accepted
//! deliberately: averaging a batch first divides the dispersion by the square
//! root of the batch size and destroys exactly the tail that p99 exists to
//! expose. It matters most for `mixed_expiration`, whose distribution is
//! bimodal by construction.
//!
//! Also reports `expiration_index_payload_bytes` (the index's element payload,
//! NOT its resident size — a `BTreeSet` adds node headers and runs partly
//! filled), `seeded_expired` (the reclaimable population the workload actually
//! established, measured once per configuration) and `slow_seed_samples` (see
//! `SEED_TTL`). Output is CSV on stdout. Run with:
//!
//! ```text
//! cargo bench -p edgion-gateway --bench lru_expiration
//! EDGION_LRU_BENCH_SMOKE=1 cargo bench -p edgion-gateway --bench lru_expiration
//! cargo bench -p edgion-gateway --bench lru_expiration --profile dev
//! ```
//!
//! The figures recorded in
//! `skills/04-review/cpu-memory/cases/lru-bounded-expiration-index.md` came from
//! the third form. A default `cargo bench` builds optimized and reports
//! materially lower absolute numbers; only the capacity-scaling ratios are
//! comparable across profiles.
//!
//! `EDGION_LRU_BENCH_SMOKE` trims the sample count and the largest capacity so
//! the benchmark stays usable as a quick correctness smoke run.

use std::hint::black_box;
use std::mem::size_of;
use std::time::{Duration, Instant};

use edgion_gateway::cache::lru::LocalLruCache;

/// Long enough that no entry seeded with it expires during a run.
const LIVE_TTL: Duration = Duration::from_secs(3_600);
/// TTL used to seed the reclaimable population of the mixed-expiration workload.
///
/// It must outlive the seeding loop itself: seeding a full cache runs the
/// ordinary full-cache insert path, which prunes every already-expired record.
/// A deadline that has already passed would therefore be drained by the very
/// next seed insert, leaving one expired entry instead of the whole batch and
/// silently degenerating the workload into `Live`. 20 ms is several orders of
/// magnitude above the observed cost of seeding a batch: 128 inserts at the
/// post-fix cost of ~2 us each is ~0.26 ms, an ~80x margin.
///
/// That margin is measured against the CURRENT implementation and does not
/// survive a before/after comparison against the pre-fix full-list walk: at its
/// published cost, seeding would take 128 x 204 us = 26 ms at capacity 16K and
/// 128 x 3.32 ms = 425 ms at 256K, both past this TTL. Anyone benchmarking a
/// slower variant must scale `SEED_TTL` first, or the batch lapses mid-seed and
/// the workload silently degenerates into `Live`. `slow_seed_samples` reports
/// how many samples had a seeding loop longer than this TTL, so the
/// degeneration is visible rather than silent.
const SEED_TTL: Duration = Duration::from_millis(20);
/// Slept after seeding so the whole seeded batch is expired before timing starts.
const SEED_SETTLE: Duration = Duration::from_millis(25);

const OPERATIONS_PER_SAMPLE: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Workload {
    /// Full cache, every entry live. Each timed insert displaces the LRU tail
    /// and must not inspect any node beyond the earliest deadline.
    Live,
    /// Full cache carrying a freshly seeded batch of expired entries.
    ///
    /// Measures bulk reclamation: `prune_expired` drains *every* expired record
    /// in one pass, so the sample's first insert pays for the whole batch, the
    /// following inserts consume the freed slots straight off the free list
    /// without pruning at all, and the remainder fall back to the full-cache
    /// path. The per-insert figure is that whole sequence averaged over the
    /// sample, which is the cost an operator actually sees across a TTL sweep.
    ///
    /// Its absolute figure is not directly comparable to `Live`: the settle
    /// sleep between samples also cools the CPU caches. Compare this workload
    /// across capacities, not against the other workloads.
    MixedExpiration,
    /// Repeated rewrites of resident keys, exercising the deadline-replacement
    /// path that drops the previous record before adding the new one.
    UpdateHeavy,
}

impl Workload {
    fn name(self) -> &'static str {
        match self {
            Workload::Live => "live",
            Workload::MixedExpiration => "mixed_expiration",
            Workload::UpdateHeavy => "update_heavy",
        }
    }
}

fn main() {
    let smoke = std::env::var_os("EDGION_LRU_BENCH_SMOKE").is_some();
    let (warmup_samples, samples) = if smoke { (5, 31) } else { (21, 201) };
    let capacities: &[usize] = if smoke {
        &[1_024, 16_384]
    } else {
        &[1_024, 16_384, 262_144]
    };

    println!(
        "schema_version,workload,capacity,arch,os,operations_per_sample,warmup_samples,samples,\
p50_ns,p95_ns,p99_ns,cache_len,expiration_index_len,expiration_index_payload_bytes,\
seeded_expired,slow_seed_samples"
    );

    for &capacity in capacities {
        for workload in [Workload::Live, Workload::MixedExpiration, Workload::UpdateHeavy] {
            measure(workload, capacity, warmup_samples, samples);
        }
    }
}

fn measure(workload: Workload, capacity: usize, warmup_samples: usize, samples: usize) {
    let cache: LocalLruCache<u64, u64> = LocalLruCache::new(capacity);
    // Fill to capacity so every timed insert runs the full-cache path.
    for key in 0..capacity as u64 {
        cache.insert(key, key, Some(LIVE_TTL));
    }

    let mut next_key = capacity as u64;
    let mut per_op_nanos = Vec::with_capacity(samples * OPERATIONS_PER_SAMPLE);
    let mut seeded_expired = 0;
    let mut slow_seed_samples = 0;

    for sample in 0..(warmup_samples + samples) {
        // Seeding is deliberately outside the timed region: it establishes the
        // starting population, it is not part of the cost under measurement.
        // A seeding loop that outlives SEED_TTL may have lapsed part of its own
        // batch, degenerating this sample toward `Live`. Timing the loop costs
        // nothing and catches the real failure mode on every sample, which a
        // once-per-configuration population count cannot. The settle sleep that
        // follows the loop is excluded — it is supposed to exceed SEED_TTL.
        let seed_elapsed = seed(workload, &cache, &mut next_key);
        if seed_elapsed >= SEED_TTL {
            slow_seed_samples += 1;
        }

        if sample == 0 {
            // Confirm the reclaimable population rather than asserting it in a
            // comment: get_keys() skips expired entries, so the shortfall
            // against capacity is the batch the timed region will reclaim. O(n),
            // so it runs once per configuration, outside the timed region.
            seeded_expired = capacity - cache.get_keys().len();
        }

        let measure_this = sample >= warmup_samples;
        run_sample(
            workload,
            &cache,
            capacity,
            &mut next_key,
            measure_this.then_some(&mut per_op_nanos),
        );
    }

    per_op_nanos.sort_unstable();
    let index_len = cache.expiration_index_len();
    println!(
        "1,{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        workload.name(),
        capacity,
        std::env::consts::ARCH,
        std::env::consts::OS,
        OPERATIONS_PER_SAMPLE,
        warmup_samples,
        samples,
        percentile(&per_op_nanos, 50),
        percentile(&per_op_nanos, 95),
        percentile(&per_op_nanos, 99),
        cache.len(),
        index_len,
        index_len * size_of::<(Instant, usize)>(),
        seeded_expired,
        slow_seed_samples,
    );
}

/// Establishes the workload's starting population, returning how long the
/// seeding loop itself took (excluding the settle sleep). `Duration::ZERO` for
/// workloads that seed nothing.
fn seed(workload: Workload, cache: &LocalLruCache<u64, u64>, next_key: &mut u64) -> Duration {
    if workload != Workload::MixedExpiration {
        return Duration::ZERO;
    }
    // Seed half a sample's worth of entries that are still live while seeding
    // runs, then let them all lapse together before the timed region starts.
    let started = Instant::now();
    for _ in 0..(OPERATIONS_PER_SAMPLE / 2) {
        cache.insert(*next_key, *next_key, Some(SEED_TTL));
        *next_key = next_key.wrapping_add(1);
    }
    let elapsed = Instant::now().saturating_duration_since(started);
    std::thread::sleep(SEED_SETTLE);
    elapsed
}

/// Runs one sample. When `timings` is `Some`, every individual insert is timed
/// and appended; warmup samples pass `None` and run the identical code path.
fn run_sample(
    workload: Workload,
    cache: &LocalLruCache<u64, u64>,
    capacity: usize,
    next_key: &mut u64,
    mut timings: Option<&mut Vec<u64>>,
) {
    let mut record = |cache: &LocalLruCache<u64, u64>, key: u64| {
        let started = Instant::now();
        cache.insert(black_box(key), key, Some(LIVE_TTL));
        let elapsed = Instant::now().saturating_duration_since(started);
        if let Some(timings) = timings.as_deref_mut() {
            timings.push(elapsed.as_nanos() as u64);
        }
    };

    match workload {
        Workload::Live | Workload::MixedExpiration => {
            for _ in 0..OPERATIONS_PER_SAMPLE {
                record(cache, *next_key);
                *next_key = next_key.wrapping_add(1);
            }
        }
        Workload::UpdateHeavy => {
            // Rewrite resident keys: no slot is allocated, but every insert
            // replaces one expiration record.
            for offset in 0..OPERATIONS_PER_SAMPLE {
                let key = (*next_key).wrapping_add(offset as u64) % capacity as u64;
                record(cache, key);
            }
            *next_key = next_key.wrapping_add(OPERATIONS_PER_SAMPLE as u64);
        }
    }
}

fn percentile(sorted_nanos: &[u64], percent: usize) -> u64 {
    if sorted_nanos.is_empty() {
        return 0;
    }
    let rank = (sorted_nanos.len() * percent).div_ceil(100).saturating_sub(1);
    sorted_nanos[rank.min(sorted_nanos.len() - 1)]
}
