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
/// read. request-level start/test timestamps are the ones that must
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

/// Keep write timestamps monotonic while the process clock remains before its
/// documented u32 wrap boundary. A stale sample has zero elapsed time and
/// cannot move the packed decay anchor backwards.
#[inline]
fn anchor_and_elapsed(now_units: u32, stored_ts: u32) -> (u32, u32) {
    let anchor = now_units.max(stored_ts);
    (anchor, anchor - stored_ts)
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
            let (anchor, elapsed) = anchor_and_elapsed(now_units, ts);
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
            // Invariant 1: anchor on the monotonic truncated timestamp.
            // Truncation errors then telescope instead of compounding per write.
            match self
                .packed
                .compare_exchange_weak(cur, pack(anchor, next), Ordering::Relaxed, Ordering::Relaxed)
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
            let (anchor, elapsed) = anchor_and_elapsed(now_units, ts);
            if penalty_us <= decay_to_default(rtt, elapsed) {
                return false;
            }
            match self
                .packed
                .compare_exchange_weak(cur, pack(anchor, penalty_us), Ordering::Relaxed, Ordering::Relaxed)
            {
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

    #[cfg(test)]
    fn stored_state(&self) -> (u32, u32) {
        unpack(self.packed.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/lb/ewma/score_test.rs"]
mod tests;
