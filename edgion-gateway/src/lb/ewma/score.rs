//! Packed, lock-free, time-decayed Peak-EWMA latency score.
//!
//! Design record: `skills/12-algorithms/lb-peak-ewma.md`. The invariants in
//! that entry's "Implementation invariants" section are load-bearing; read them
//! before changing anything here.
//!
//! One `AtomicU64` holds a consistent (timestamp, value) pair:
//!
//! ```text
//! [63:32] ts_units : u32   monotonic, 16 ms granularity (~2.2 years to wrap)
//! [31:0]  rtt_us   : u32   saturating (~71.6 minutes)
//! ```
//!
//! Packing avoids both a lock and a torn read across two atomics. Decay is
//! applied lazily on read, toward `DEFAULT_RTT_US` rather than toward zero, so
//! "no information" scores neutral instead of winning.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::Instant;

/// Process-monotonic epoch. All timestamps are unit offsets from this, which
/// keeps the packed field a u32 and keeps the clock monotonic — no wall-clock
/// read ever reaches the score.
static PROCESS_START: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Timestamp units for an `Instant` the caller already holds.
///
/// Used where a suitable `Instant` genuinely exists for *this moment* — the
/// response filter reuses the one it already took for header-time accounting.
///
/// The selection path calls [`now_units`] instead. `upstream.start_time` is
/// refreshed per attempt so threading it would also be correct; reading fresh
/// just avoids plumbing an `Instant` through four call sites for a ~20 ns vDSO
/// read. `ctx.start_time` / `ctx.upstream_start_time` are the ones that must
/// never be used — request-scoped, and stale by multiple half-lives on a
/// retried or long-lived streaming request.
#[inline]
pub fn units_at(t: Instant) -> u32 {
    to_units(t.saturating_duration_since(*PROCESS_START).as_millis() as u64)
}

/// Timestamp units for right now. Use `units_at` when an `Instant` is already
/// available.
#[inline]
pub fn now_units() -> u32 {
    units_at(Instant::now())
}

/// Timestamp granularity. 16 ms keeps the quantization error at ~0.11% against
/// a 10 s half-life while giving a u32 ~2.2 years of headroom.
pub const TS_UNIT_MS: u64 = 16;

/// Half-life of the decay, in timestamp units. 10 s — corroborated by
/// Linkerd's `decay` default and Kong's `DECAY_TIME`. **Benchmark-gated**, and
/// shared with the overload penalty's bleed-off rate: it cannot be tuned for
/// latency alone. See `skills/12-algorithms/lb-overload-penalty.md`.
pub const HALF_LIFE_UNITS: u32 = 625; // 10_000 ms / 16 ms

/// Score a backend relaxes to when it carries no information, in µs.
///
/// **Benchmark-gated.** 30 ms follows Linkerd's `default_rtt`. The previous
/// fixed-alpha estimator seeded 1 ms, which under peak semantics plus the
/// outstanding-request multiplier over-favors new and idle backends: a fresh
/// backend would look ~30x better than a healthy one and absorb the burst.
pub const DEFAULT_RTT_US: u32 = 30_000;

/// Past this many whole half-lives the decayed value is indistinguishable from
/// `DEFAULT_RTT_US`, so the read short-circuits instead of shifting.
///
/// **This is also a hard legality bound, with zero margin.** `observe` shifts
/// the Q16 factor as `q16 >> whole` on a **u32**, so `whole <= 31` is required
/// or that line panics in debug and is UB in release. Raising this constant
/// past 32 breaks there first — not on the u64 shifts in `decay_to_default`,
/// which have room to 47. Do not raise it without widening that shift.
const MAX_WHOLE_HALF_LIVES: u32 = 32;

/// Resolution of the fractional-half-life table.
const FRAC_STEPS: u32 = 32;

/// `2^(-i/32)` in Q16, for `i` in `0..=32`. Removes `f64`/libm from the read
/// path: one table index, one interpolation, one multiply, two shifts.
///
/// The trailing 33rd entry is `2^-1` exactly, so the interpolation below always
/// has a right-hand neighbour without a bounds check.
const DECAY_Q16: [u32; FRAC_STEPS as usize + 1] = [
    65536, 64132, 62757, 61413, 60097, 58809, 57549, 56316, 55109, 53928, 52773, 51642, 50535, 49452, 48393, 47356,
    46341, 45348, 44376, 43425, 42495, 41584, 40693, 39821, 38968, 38133, 37316, 36516, 35734, 34968, 34219, 33486,
    32768,
];

/// Convert a monotonic millisecond reading into timestamp units.
#[inline]
pub fn to_units(millis: u64) -> u32 {
    (millis / TS_UNIT_MS) as u32
}

#[inline]
fn pack(ts_units: u32, rtt_us: u32) -> u64 {
    ((ts_units as u64) << 32) | rtt_us as u64
}

#[inline]
fn unpack(raw: u64) -> (u32, u32) {
    ((raw >> 32) as u32, raw as u32)
}

/// Decay factor `2^(-elapsed / half_life)` in Q16, plus the whole-half-life
/// shift to apply afterwards. Returns `None` once the value is fully relaxed.
///
/// The fractional weight is **linearly interpolated** between table entries.
/// This is not a refinement — it is required for correctness. One table index
/// spans `HALF_LIFE_UNITS / FRAC_STEPS = 19.53` units = **312.5 ms**, twenty
/// times the 16 ms timestamp granularity. Truncating to the table index instead
/// of interpolating makes `d` exactly `1.0` for every elapsed under 320 ms,
/// which means zero decay *and* zero weight for a new sample. Since `observe`
/// re-anchors the timestamp on every write, any backend receiving traffic more
/// often than every 320 ms would then never decay at all: its score would
/// ratchet monotonically to its worst-ever latency and pin there for the life
/// of the process — the absorbing state this estimator exists to remove,
/// sustained by traffic rather than by starvation. Regression:
/// `spiked_backend_recovers_under_sustained_traffic` and
/// `decay_is_rate_independent`.
#[inline]
fn decay_parts(elapsed_units: u32) -> Option<(u32, u32)> {
    let whole = elapsed_units / HALF_LIFE_UNITS;
    if whole >= MAX_WHOLE_HALF_LIVES {
        return None;
    }
    let rem = elapsed_units % HALF_LIFE_UNITS;
    // `rem <= 624`, so `num <= 19_968` — no overflow, and `idx <= 31`, leaving
    // `idx + 1` in bounds against the 33-entry table.
    let num = rem as u64 * FRAC_STEPS as u64;
    let idx = (num / HALF_LIFE_UNITS as u64) as usize;
    let frac = num % HALF_LIFE_UNITS as u64;
    let a = DECAY_Q16[idx] as u64;
    let b = DECAY_Q16[idx + 1] as u64;
    let q16 = (a - (a - b) * frac / HALF_LIFE_UNITS as u64) as u32;
    Some((q16, whole))
}

/// Apply decay to `stored`, pulling it toward `DEFAULT_RTT_US`.
///
/// Works from either side: a backend scoring better than default decays *up*
/// toward it, exactly as one scoring worse decays down. 64-bit intermediates
/// keep the Q16 multiply exact — `|v - default| * 65536` reaches ~2.8e14, which
/// overflows u32 but not u64.
#[inline]
fn decay_to_default(stored: u32, elapsed_units: u32) -> u32 {
    let Some((q16, whole)) = decay_parts(elapsed_units) else {
        return DEFAULT_RTT_US;
    };
    // Compute on the magnitude and reapply the sign, so the truncation is
    // always toward `DEFAULT_RTT_US` on both sides. Operating on a signed
    // delta would not do this: `>>` on a negative i64 floors toward negative
    // infinity, which *grows* the magnitude and biases a below-default backend
    // away from neutral — i.e. it would inflate a fast backend's advantage.
    // Error is at most 1 µs per evaluation. It does not compound across
    // `read`, which is a pure projection — but it DOES compound through
    // `observe`, which stores `eff`: the signed form leaves a fast-then-idle
    // backend pinned ~923 µs (3.1% of DEFAULT_RTT_US) below neutral under
    // sustained traffic. Regression: `below_default_decay_is_biased_toward_neutral`.
    let below = stored < DEFAULT_RTT_US;
    let magnitude = (stored as i64 - DEFAULT_RTT_US as i64).unsigned_abs();
    let decayed = (((magnitude * q16 as u64) >> 16) >> whole) as i64;
    let signed = if below { -decayed } else { decayed };
    (DEFAULT_RTT_US as i64 + signed).clamp(0, u32::MAX as i64) as u32
}

/// Per-backend Peak-EWMA score.
#[derive(Debug)]
pub struct PeakEwmaScore {
    packed: AtomicU64,
}

impl PeakEwmaScore {
    /// Seed at `DEFAULT_RTT_US` so a new backend is neutral, not winning.
    pub fn new(now_units: u32) -> Self {
        Self {
            packed: AtomicU64::new(pack(now_units, DEFAULT_RTT_US)),
        }
    }

    /// Current decayed estimate in µs. One relaxed load; no write-back, so
    /// concurrent readers never contend and the read is a pure projection.
    #[inline]
    pub fn read(&self, now_units: u32) -> u32 {
        let (ts, rtt) = unpack(self.packed.load(Ordering::Relaxed));
        decay_to_default(rtt, now_units.saturating_sub(ts))
    }

    /// Record a latency sample with peak-immediate semantics: a sample worse
    /// than the current estimate is adopted whole, a better one is blended with
    /// the elapsed-derived decay factor as its weight.
    ///
    /// The blend weight **is** the decay factor, which is what deletes the old
    /// global `alpha`: a high-QPS backend (small elapsed, `d ~= 1`) smooths
    /// heavily, while a long-idle one (`d ~= 0`) adopts the sample outright.
    ///
    /// Consequence worth knowing: at zero elapsed the sample's weight is
    /// `1 - d = 0`, so a *faster* sample arriving within the same 16 ms tick is
    /// discarded entirely. Only peaks register at high QPS, and recovery is
    /// carried by time-decay toward `DEFAULT_RTT_US` rather than by fast
    /// samples. Linkerd has the same property but hides it behind nanosecond
    /// timestamps. Do not "fix" this by floor-ing the sample weight: that
    /// reintroduces exactly the QPS-dependent smoothing this design removes.
    pub fn observe(&self, sample_us: u32, now_units: u32) {
        let mut cur = self.packed.load(Ordering::Relaxed);
        loop {
            let (ts, rtt) = unpack(cur);
            let elapsed = now_units.saturating_sub(ts);
            // Invariant 2: both the decayed read and the value to store are
            // recomputed from the value observed on THIS iteration. Hoisting
            // either above the loop can stomp a concurrent higher peak.
            let eff = decay_to_default(rtt, elapsed);
            let next = if sample_us >= eff {
                sample_us
            } else {
                match decay_parts(elapsed) {
                    // eff * d + sample * (1 - d), in Q16.
                    Some((q16, whole)) => {
                        let d = (q16 >> whole) as u64;
                        let blended = (eff as u64 * d + sample_us as u64 * (65536 - d)) >> 16;
                        blended.min(u32::MAX as u64) as u32
                    }
                    // Fully relaxed: no history left to weigh against.
                    None => sample_us,
                }
            };
            // Invariant 1: anchor on the truncated `now`. Truncation errors
            // then telescope instead of compounding per write.
            match self
                .packed
                .compare_exchange_weak(cur, pack(now_units, next), Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return,
                Err(actual) => cur = actual,
            }
        }
    }

    /// Store a synthetic sample **only if it would raise the estimate**.
    ///
    /// Returns `true` if it was stored. A value that would not be a peak is
    /// skipped entirely — no store, and **no timestamp advance**, so the decay
    /// anchor keeps running from the last real sample and this path contributes
    /// nothing to per-write rounding drift.
    ///
    /// Routing such a value through [`observe`](Self::observe) instead would
    /// take the blend branch and *lower* a slow endpoint's score toward it,
    /// which is the opposite of the intent in both current uses.
    ///
    /// Two callers, one mechanism, different inputs:
    ///
    /// * **Non-useful-work responses** (5xx, and any status the route declares
    ///   retryable) pass their raw measured time-to-headers, clamped by
    ///   `crate::lb::sample::MAX_ERROR_SAMPLE_US`. A slow failure penalizes; a
    ///   fast one must not reward. See `skills/12-algorithms/lb-peak-ewma.md`.
    /// * **Provider overload penalties** (planned) will pass a bounded
    ///   synthetic duration derived from `Retry-After`. See
    ///   `skills/12-algorithms/lb-overload-penalty.md`.
    ///
    /// The clamping is the caller's responsibility; this method bounds nothing.
    #[must_use = "the caller may need to know whether the peak was actually stored"]
    pub fn observe_peak_only(&self, penalty_us: u32, now_units: u32) -> bool {
        let mut cur = self.packed.load(Ordering::Relaxed);
        loop {
            let (ts, rtt) = unpack(cur);
            if penalty_us <= decay_to_default(rtt, now_units.saturating_sub(ts)) {
                return false;
            }
            match self.packed.compare_exchange_weak(
                cur,
                pack(now_units, penalty_us),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(actual) => cur = actual,
            }
        }
    }

    /// Raw stored value, for tests and metric snapshots. Not decayed.
    #[cfg(test)]
    fn stored(&self) -> u32 {
        unpack(self.packed.load(Ordering::Relaxed)).1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: u32 = HALF_LIFE_UNITS;

    #[test]
    fn seeds_at_default_rtt() {
        let s = PeakEwmaScore::new(0);
        assert_eq!(s.read(0), DEFAULT_RTT_US);
    }

    #[test]
    fn slower_sample_is_adopted_whole() {
        let s = PeakEwmaScore::new(0);
        s.observe(500_000, 0);
        assert_eq!(s.read(0), 500_000, "peak semantics adopt a worse sample immediately");
    }

    #[test]
    fn faster_sample_blends_rather_than_replacing() {
        let s = PeakEwmaScore::new(0);
        s.observe(500_000, 0);
        s.observe(1_000, H / 2);
        let v = s.read(H / 2);
        // The estimate is decayed toward default FIRST, then blended — the
        // blend's base is `eff`, not the raw stored value. Table + Q16 values:
        //   eff = 362_527, out = 256_781.
        assert!(
            (v as i64 - 256_781).abs() < 500,
            "expected the blend near 256_781, got {v}"
        );
    }

    #[test]
    fn decays_toward_default_from_above() {
        let s = PeakEwmaScore::new(0);
        s.observe(DEFAULT_RTT_US * 8, 0);
        let one = s.read(H);
        let three = s.read(3 * H);
        assert!(one < DEFAULT_RTT_US * 8 && one > DEFAULT_RTT_US);
        assert!(three < one, "decay must be monotone toward default");
        // 8x default is 3 half-lives above it: 8D -> D + 7D/8 ~= 1.875D.
        // 8D decays to D + 7D*2^-3 = 1.875D. Tolerance 1% of D, not 25%: a
        // looser bound hides table-resolution errors.
        assert!(
            (three as i64 - (DEFAULT_RTT_US as i64 * 15 / 8)).abs() < DEFAULT_RTT_US as i64 / 100,
            "three half-lives from 8x default should land near 1.875x, got {three}"
        );
    }

    #[test]
    fn decays_toward_default_from_below() {
        let s = PeakEwmaScore::new(0);
        // Needs elapsed > 0: at zero elapsed a faster sample carries no weight
        // (see `faster_sample_at_zero_elapsed_is_ignored`).
        s.observe(1_000, H);
        assert!(s.read(H) < DEFAULT_RTT_US);
        let later = s.read(H + 4 * H);
        assert!(
            later > s.read(H) && later <= DEFAULT_RTT_US,
            "a fast-but-idle backend must relax UP to neutral, not stay winning"
        );
    }

    #[test]
    fn faster_sample_at_zero_elapsed_is_ignored() {
        // The smoothing weight IS the decay factor, so at elapsed = 0 the new
        // sample's weight is 1 - d = 0. Linkerd behaves identically (its `add`
        // early-returns when `ts <= self.timestamp`). Because our timestamps
        // are quantized to 16 ms rather than nanoseconds, this bites a whole
        // tick's worth of samples, not just simultaneous ones: at high QPS only
        // peaks register and recovery is carried by time-decay rather than by
        // fast samples. That is the designed recovery path, so it is accepted —
        // but it must not be "fixed" by giving zero-elapsed samples weight,
        // which would reintroduce QPS-dependent smoothing.
        let s = PeakEwmaScore::new(0);
        s.observe(500_000, 0);
        s.observe(1_000, 0);
        assert_eq!(s.read(0), 500_000, "a faster sample in the same tick has zero weight");
        // A slower sample in the same tick still wins, via the peak branch.
        s.observe(900_000, 0);
        assert_eq!(s.read(0), 900_000);
    }

    #[test]
    fn spiked_backend_recovers_without_traffic() {
        // The regression test the fixed-alpha absorbing state never had: a
        // backend spikes once, is then never selected again, and must still
        // return to neutral on its own.
        let s = PeakEwmaScore::new(0);
        s.observe(30_000_000, 0); // a 30 s SSE-shaped sample
        assert!(s.read(0) > DEFAULT_RTT_US * 100);
        let recovered = s.read(40 * H);
        assert_eq!(recovered, DEFAULT_RTT_US, "must fully relax with no new samples");
    }

    #[test]
    fn read_is_a_pure_projection() {
        let s = PeakEwmaScore::new(0);
        s.observe(500_000, 0);
        let before = s.stored();
        let _ = s.read(5 * H);
        assert_eq!(s.stored(), before, "read must not write back");
    }

    #[test]
    fn peak_only_stores_only_when_it_is_a_peak() {
        let s = PeakEwmaScore::new(0);
        assert!(s.observe_peak_only(5_000_000, 0), "5 s penalty beats a 30 ms score");
        assert_eq!(s.read(0), 5_000_000);
        assert!(
            !s.observe_peak_only(5_000_000, 0),
            "an identical penalty at the same instant is not a peak and must be skipped"
        );
        assert!(!s.observe_peak_only(1_000_000, 0), "a smaller penalty must be skipped");
        // Once the score has decayed below it, the same penalty IS a peak again
        // and re-stores — this is the documented converge-and-pin behaviour.
        assert!(
            s.observe_peak_only(5_000_000, H),
            "after decay the same penalty is a strict peak again"
        );
        assert_eq!(s.read(H), 5_000_000);
    }

    #[test]
    fn skipped_penalty_does_not_advance_the_decay_anchor() {
        let s = PeakEwmaScore::new(0);
        s.observe(20_000_000, 0);
        let expected = s.read(2 * H);
        assert!(
            !s.observe_peak_only(5_000_000, 2 * H),
            "20 s score outranks a 5 s penalty"
        );
        assert_eq!(
            s.read(2 * H),
            expected,
            "a skipped penalty must leave both value and anchor untouched"
        );
    }

    #[test]
    fn repeated_penalties_converge_down_and_pin() {
        // Idempotent, not additive: a storm on an already-slow endpoint walks
        // the score DOWN to the penalty and stops there.
        let s = PeakEwmaScore::new(0);
        s.observe(20_000_000, 0);
        let mut prev = s.read(0);
        for k in 1..=40 {
            let now = k * H / 2;
            // Once the score has decayed below the penalty, the same value is a
            // strict peak again and re-stores; while it is above, the write is
            // skipped. Both outcomes are correct here, hence the discard.
            let _ = s.observe_peak_only(5_000_000, now);
            let v = s.read(now);
            assert!(v <= prev + 1, "must never increase: {prev} -> {v}");
            assert!(v >= 5_000_000 - 1, "must never fall below the penalty: {v}");
            prev = v;
        }
        assert!(
            (prev as i64 - 5_000_000).abs() < 100_000,
            "should pin at the penalty, got {prev}"
        );
    }

    #[test]
    fn saturates_instead_of_wrapping() {
        let s = PeakEwmaScore::new(0);
        s.observe(u32::MAX, 0);
        assert_eq!(s.read(0), u32::MAX);
        // And still decays back down rather than sticking.
        assert_eq!(s.read(40 * H), DEFAULT_RTT_US);
    }

    #[test]
    fn concurrent_observes_keep_the_peak() {
        use std::sync::Arc;
        let s = Arc::new(PeakEwmaScore::new(0));
        let mut hs = Vec::new();
        for i in 0..8u32 {
            let s = Arc::clone(&s);
            hs.push(std::thread::spawn(move || {
                for _ in 0..2_000 {
                    s.observe(100_000 + i, 0);
                }
            }));
        }
        for h in hs {
            h.join().unwrap();
        }
        let v = s.read(0);
        assert_eq!(
            v, 100_007,
            "a lost CAS retry must not stomp a concurrent higher peak, got {v}"
        );
    }

    #[test]
    fn decay_table_entries_are_exact() {
        // Assert the real property rather than a proxy: every entry is the
        // correctly-rounded 2^(-i/32) in Q16, and the last is 2^-1 exactly.
        for (i, &v) in DECAY_Q16.iter().enumerate() {
            let expect = (65536.0f64 * 2f64.powf(-(i as f64) / 32.0)).round() as u32;
            assert_eq!(v, expect, "DECAY_Q16[{i}] must be round(65536 * 2^(-{i}/32))");
        }
        assert_eq!(DECAY_Q16[0], 65536);
        assert_eq!(DECAY_Q16[FRAC_STEPS as usize], 32768, "last entry must be exactly 2^-1");
        for w in DECAY_Q16.windows(2) {
            assert!(w[1] < w[0], "table must be strictly decreasing");
        }
    }

    #[test]
    fn decay_is_representable_below_one_table_step() {
        // One table index spans 312.5 ms. Without interpolation every elapsed
        // under 320 ms decayed by exactly zero, which — combined with the
        // per-write anchor refresh — froze any backend under traffic.
        let s = PeakEwmaScore::new(0);
        s.observe(500_000, 0);
        let mut prev = s.read(0);
        for el in 1..=20u32 {
            let v = s.read(el);
            assert!(
                v < prev,
                "read must strictly decrease at elapsed={el} units ({} ms)",
                el * 16
            );
            prev = v;
        }
    }

    #[test]
    fn spiked_backend_recovers_under_sustained_traffic() {
        // The regression test the first implementation lacked: recovery must
        // work for a backend that is STILL RECEIVING TRAFFIC, not only an idle
        // one. Every 16 ms (~60 rps) is well inside the old dead zone.
        let s = PeakEwmaScore::new(0);
        s.observe(500_000, 0);
        let mut t = 0u32;
        while t < 12 * H {
            t += 1;
            s.observe(5_000, t);
        }
        let v = s.read(t);
        assert!(
            v < 20_000,
            "a backend under sustained 5 ms traffic must recover from a 500 ms spike, got {v}"
        );
    }

    #[test]
    fn decay_is_rate_independent() {
        // Defect 3's regression test: forgetting is driven by elapsed time, not
        // by request count. The intervals stay in the high-`d` band
        // (`d ∈ [0.87, 0.999]`) where the below-default fixed point is flattest;
        // they do not claim `v*` is independent of rate at 0.1 rps. See
        // `skills/12-algorithms/lb-peak-ewma.md` known limitation 6.
        // Under the old fixed-alpha estimator this was false by construction,
        // and under the un-interpolated table the fast case did not converge.
        let span = 8 * H;
        let mut scores = Vec::new();
        for interval in [1u32, 6, 31, 125] {
            let s = PeakEwmaScore::new(0);
            s.observe(500_000, 0);
            let mut t = 0u32;
            while t + interval <= span {
                t += interval;
                s.observe(5_000, t);
            }
            scores.push(s.read(span));
        }
        let (lo, hi) = (
            *scores.iter().min().unwrap() as i64,
            *scores.iter().max().unwrap() as i64,
        );
        assert!(
            hi - lo < 6_000,
            "decay must not depend on request rate; got spread {lo}..{hi} for {scores:?}"
        );
    }

    #[test]
    fn below_default_decay_is_biased_toward_neutral() {
        // Truncation must always shrink the distance to DEFAULT_RTT_US, on both
        // sides. A signed-shift implementation floors toward negative infinity,
        // which GROWS the magnitude and pushes a fast backend further below
        // neutral.
        //
        // The bound checks at the end are NOT enough to pin that: `read` is a
        // pure projection, so both implementations produce a monotone sequence
        // bounded by DEFAULT_RTT_US and differ by only 1 µs. Two assertions
        // that do pin it:
        //
        // (a) Exact value one tick after a below-default store. Magnitude
        //     arithmetic gives 15_017; the signed shift floors toward negative
        //     infinity and yields 15_016. Note the store needs elapsed > 0 —
        //     at zero elapsed a faster sample carries zero weight by design
        //     (see `faster_sample_at_zero_elapsed_is_ignored`).
        let s = PeakEwmaScore::new(0);
        s.observe(0, H);
        assert_eq!(
            s.read(H),
            15_000,
            "blend at one half-life halves the distance to neutral"
        );
        assert_eq!(
            s.read(H + 1),
            15_017,
            "sign must be reapplied after truncating the magnitude, not folded into a signed shift"
        );

        // (b) Chained through `observe`, which stores `eff` and therefore does
        //     compound the per-write bias. Reading 40 half-lives later is a
        //     tautology — both implementations fully relax. Pin the projection
        //     at the last write: magnitude-then-sign lands at 15_231; a signed
        //     shift leaves ~14_752. Under sustained `observe(0)` the high-QPS
        //     fixed point is (D + sample)/2 ≈ 15_000, not DEFAULT_RTT_US.
        let s = PeakEwmaScore::new(0);
        s.observe(1_000, 0);
        let mut t = 0u32;
        for _ in 0..4_000 {
            t += 1;
            s.observe(0, t);
        }
        assert_eq!(
            s.read(t),
            15_231,
            "chained below-default observe must keep magnitude-then-sign truncation"
        );

        // (c) The original bounds, retained as an invariant check.
        let s = PeakEwmaScore::new(0);
        s.observe(1_000, H);
        let base = s.read(H);
        for el in 1..40u32 {
            let v = s.read(H + el * H / 8);
            assert!(v >= base, "a below-default score must only move up toward neutral");
            assert!(v <= DEFAULT_RTT_US, "and never overshoot past neutral, got {v}");
        }
    }

    #[test]
    fn out_of_order_timestamps_do_not_panic_or_corrupt() {
        let s = PeakEwmaScore::new(0);
        s.observe(200_000, 100);
        s.observe(5_000, 99);
        let v = s.read(100);
        assert!(
            v <= 200_000,
            "a backwards timestamp must not inflate the score, got {v}"
        );

        // A peak-only store DOES move the anchor backwards when its own
        // timestamp is older than the stored one. That is deliberate and
        // fail-safe: subsequent reads then see a LARGER elapsed and decay
        // harder, i.e. toward neutral. Assert the direction rather than
        // leaving it unexamined — the previous form here was a short-circuiting
        // tautology that could not fail.
        let s = PeakEwmaScore::new(0);
        s.observe(100_000, 100);
        let before_backwards = s.read(200);
        assert!(
            s.observe_peak_only(400_000, 50),
            "400 ms is a strict peak over the decayed 100 ms"
        );
        assert_eq!(s.read(50), 400_000, "the peak is stored at its own timestamp");
        let after = s.read(200);
        assert!(
            after < 400_000 && after > before_backwards,
            "a backwards anchor must decay harder, not freeze: {before_backwards} -> {after}"
        );
    }

    #[test]
    fn fully_relaxed_backend_adopts_a_faster_sample_whole() {
        // The `whole >= MAX_WHOLE_HALF_LIVES` branch: no history left to weigh.
        let s = PeakEwmaScore::new(0);
        s.observe(500_000, 0);
        s.observe(1_000, 40 * H);
        assert_eq!(s.read(40 * H), 1_000);
    }
}
