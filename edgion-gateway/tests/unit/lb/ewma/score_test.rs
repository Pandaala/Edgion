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
fn stale_observe_keeps_anchor_and_latest_peak() {
    let s = PeakEwmaScore::new(0);
    s.observe(200_000, 100);
    s.observe(400_000, 99);
    assert_eq!(
        s.stored_state(),
        (100, 400_000),
        "a stale higher sample must win without moving the anchor backwards"
    );

    s.observe(5_000, 98);
    assert_eq!(
        s.stored_state(),
        (100, 400_000),
        "a stale faster sample has zero weight and must preserve the latest peak"
    );
}

#[test]
fn stale_peak_only_keeps_anchor_and_peak_dominance() {
    let s = PeakEwmaScore::new(0);
    s.observe(100_000, 100);
    assert!(
        s.observe_peak_only(400_000, 50),
        "a stale strict peak must still be stored"
    );
    assert_eq!(s.stored_state(), (100, 400_000));

    assert!(
        !s.observe_peak_only(400_000, 49),
        "an equal stale penalty is not a strict peak"
    );
    assert_eq!(s.stored_state(), (100, 400_000));
    assert!(
        !s.observe_peak_only(300_000, 48),
        "a lower stale penalty must be skipped"
    );
    assert_eq!(s.stored_state(), (100, 400_000));

    assert!(s.observe_peak_only(500_000, 47));
    assert_eq!(
        s.stored_state(),
        (100, 500_000),
        "the latest strict peak must win without regressing the anchor"
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
