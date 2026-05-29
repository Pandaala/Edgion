---
name: rr-consistent-hash-per-backend-weight-cap
description: RoundRobinSelector::build and ConsistentHashRing::build now cap the per-backend weight before expanding it into an allocation. Reject re-reports claiming an unbounded per-backend weight blows up the RR index array or the ketama ring once per-endpoint weights land.
---

# lb-F013 — RR / ConsistentHash selectors cap per-backend weight expansion

**Status:** ✅ fixed · 2026-05-25
**Severity:** Low (BadLogic / resource exhaustion) — **dormant** when fixed: per-endpoint
weights are hard-coded to `1` today, so neither path could be triggered in production. Fixed
proactively as defense-in-depth so the guard is already in place the moment per-endpoint
weights become configurable.

## Conclusion

Both data-plane selector build paths now bound the per-backend weight before it feeds an
allocation:

- `RoundRobinSelector::build` (`src/core/gateway/lb/selection/round_robin.rs`) runs
  `normalize_rr_weights` on the per-backend weights (each `.max(1)`) before expanding them into
  the `weighted: Box<[u16]>` index array. The helper (1) reduces by GCD (lossless, collapses
  `[100, 100]` → `[1, 1]`) then (2) if the total still exceeds `MAX_RR_WEIGHTED_LEN = 1 << 20`,
  scales every weight down proportionally with `.max(1)` so no backend drops out. The expanded
  array is therefore bounded to ~`MAX_RR_WEIGHTED_LEN + backend_count` entries (~2 MiB worst
  case) instead of O(sum of weights).
- `ConsistentHashRing::build` (`src/core/gateway/lb/selection/consistent_hash.rs`) clamps each
  weight via `b.weight.clamp(1, MAX_CH_BACKEND_WEIGHT) as u32` (`MAX_CH_BACKEND_WEIGHT = 1 << 10`)
  before `Bucket::new`. This both removes the silent `usize -> u32` truncation (an unclamped
  value above `u32::MAX` previously wrapped) and bounds the ~160×weight ketama virtual-node
  count per backend.

Re-reports of the form "a single backend with a very large `weight` allocates a multi-GB
`Vec<u16>` in `RoundRobinSelector::build`", "the `b.weight.max(1) as u32` cast truncates /
explodes the ketama ring", or "the lb-F005 fix doesn't cover `RoundRobinSelector`" are **not
accepted** — verify the current `build` paths call `normalize_rr_weights` / `clamp(1,
MAX_CH_BACKEND_WEIGHT)` before re-reporting.

## What was wrong (before the fix)

`RoundRobinSelector::build` expanded each backend with `for _ in 0..b.weight.max(1) { weighted.push(..) }`
and `ConsistentHashRing::build` passed `b.weight.max(1) as u32` straight to `Bucket::new`.
The backend *count* was bounded (RR truncates to `u16::MAX`; the ring excludes non-Inet
addrs), but the *per-backend weight* was not. lb-F005 capped the sibling `WeightedRoundRobin`
(`backend_selector/weighted_selector.rs`) — a **different** selector. The selector actually
used by the data-plane `RrCore` / `ChCore` (`compiled_selector.rs`) is this `RoundRobinSelector`
/ `ConsistentHashRing` pair, so the lb-F005 fix did **not** extend here.

The defect was dormant because the EndpointSlice discovery path hard-codes `weight: 1`
(`discovery/endpoint_slice/discovery_impl.rs`) and `replace_backends` (`compiled_selector.rs`)
explicitly carries over the old `weight=1` Arc. It would have become live the moment
per-endpoint weights are introduced.

## Why the fix is correct

- **Zero behavior change today.** With every weight `== 1`: GCD of all-ones is 1 (no
  reduction), the RR total equals the backend count (`<= u16::MAX < MAX_RR_WEIGHTED_LEN`, no
  scaling), and `clamp(1, 1024)` returns 1. The expanded RR array and the ketama ring are
  byte-for-byte identical to the pre-fix output.
- **Ratio-preserving.** RR fairness and ketama share depend only on the *ratio* between
  weights, not their magnitude; GCD reduction is lossless and proportional scaling preserves
  the ratio (mirrors lb-F005's reasoning).
- **Origin, not symptom.** Both fixes live in the `build` constructors (run at selector
  materialization on backend replacement, never on the per-request `select` hot path), so all
  current and future callers inherit the bound.

## Fix Suggestions Not Accepted

- "Reuse `weighted_selector::normalize_weights` directly for RR" — its `MAX_SMOOTH_SEQUENCE_LEN
  = 10_000` cap is smaller than the RR backend ceiling (`u16::MAX = 65_535`), so it would drop
  backends from the rotation. RR needs its own larger `MAX_RR_WEIGHTED_LEN`.
- "Only saturate the cast in `ConsistentHashRing::build`, skip bounding the vnode count" — a
  saturating cast alone still lets a `u32::MAX` weight expand into ~160×`u32::MAX` virtual
  nodes; the clamp bounds both.
- "Leave it dormant / close as Won't-Fix until per-endpoint weights land" — the guard is cheap,
  a no-op today, and being in place avoids the silent-becomes-live risk when weights are added
  (the same proactive-defense posture as lb-F005 / lb-F001).

## Re-evaluation Triggers

Re-open / revisit only if:

- Either `build` path reverts to expanding `b.weight.max(1)` without `normalize_rr_weights` /
  `clamp`.
- Per-endpoint weights are introduced with a documented maximum that makes `MAX_RR_WEIGHTED_LEN`
  / `MAX_CH_BACKEND_WEIGHT` distort legitimate ratios (then tune the constants, do not remove).

## Reference Cases

- lb-F013 (review finding, dormant follow-up promoted from `tasks/todo/`, 2026-05-25)
- Sibling: lb-F005 (`WeightedRoundRobin` per-backend weight cap), lb-F001 (EWMA integer
  widening) — same "unbounded integer feeds an allocation/multiply" class.
- Unit regressions: `round_robin.rs` — `huge_per_backend_weight_is_capped_and_light_backend_survives`,
  `common_factor_weights_are_reduced`; `consistent_hash.rs` —
  `huge_weight_is_clamped_and_ring_stays_bounded`.
- `compiled_selector.rs` — `RrCore::build` / `ChCore::build`, the only non-test callers.
