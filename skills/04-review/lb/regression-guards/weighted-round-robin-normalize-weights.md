---
name: weighted-round-robin-normalize-weights
description: Use when reviewing WeightedRoundRobin::new O(sum-of-weights) allocation / unclamped BackendRef.weight OOM findings — new() now runs normalize_weights (GCD reduce + proportional scale to MAX_SMOOTH_SEQUENCE_LEN) before generate_smooth_sequence; reject the O(sum-of-weights) / unclamped-weight form.
status: fixed
finding_id: lb-F005
closed: 2026-05-24
knowledge_type: regression-guards
---

# WeightedRoundRobin::new O(sum-of-weights) Sequence Allocation / Unclamped Weight

**Status / verdict**: fixed (lb-F005, 2026-05-24)

When reviewing findings of the form "`WeightedRoundRobin::new` builds an O(sum-of-weights) smooth sequence / `BackendRef.weight` is unclamped (up to `i32::MAX`) so two large weights allocate a multi-GB `Vec<u16>` → OOM-kill / the precomputed sequence is unbounded (lb-F005)": **fixed (lb-F005, 2026-05-24)**.

`WeightedRoundRobin::new` (`backend_selector/weighted_selector.rs`) now runs `normalize_weights` before `generate_smooth_sequence`, so the precomputed `Box<[u16]>` sequence can no longer be O(sum of weights). `BackendRef.weight` is `Option<i32>` (up to `i32::MAX`) with no upstream clamp, and `selector.rs::create_state` converted it with `w as usize` + `w > 0` filter only; two backends near `i32::MAX` would allocate a ~4.29e9-entry `Vec<u16>` (~8.6 GB) on the first request after route (re)load → OOM-kill, reachable via a routine HTTPRoute/GRPCRoute RBAC edit. `normalize_weights` (1) reduces by GCD — lossless, `[N,N]`→`[1,1]`, `[100,200]`→`[1,2]` — then (2) if the total still exceeds `MAX_SMOOTH_SEQUENCE_LEN = 10_000`, scales every positive weight down proportionally with `.max(1)` so no backend drops out. Ratio (the only thing WRR fairness depends on) is preserved; resolution bounded to 1/10,000; worst-case sequence ~20 KB. Fixed at the constructor (origin) not `selector.rs:91` (symptom), so `with_equal_weights` (webhook targets) and all future callers inherit it; the `select()` hot path is untouched (normalization runs only at construction).

Chose proportional scaling over switching to an Envoy-style EDF scheduler because EDF needs a mutable heap (lock per pick) and would abandon this module's deliberate lock-free O(1) design. Unit tests in `weighted_selector.rs`: `gcd_basic`, `normalize_*` (gcd reduction / huge-equal-collapse / coprime-scale-within-budget / zero-preserve), `new_bounds_sequence_for_coprime_huge_weights`, `new_huge_equal_weights_still_distribute_evenly`. Doc: `http-route/lb-algorithms.md` weight-as-ratio note.

Sibling: lb-F013 (`RoundRobinSelector` / `ConsistentHashRing` per-backend weight cap) — lb-F005 capped this `WeightedRoundRobin` constructor; lb-F013 covers the selector pair that lb-F005 did not extend to. Same "unbounded integer feeds an allocation/multiply" class as lb-F001 (EWMA integer).

Re-reports of the O(sum-of-weights) allocation / unclamped weight form are not accepted; verify `WeightedRoundRobin::new` calls `normalize_weights` before re-reporting.

## Historical resolution evidence: lb-F005

`WeightedRoundRobin::new` (`backend_selector/weighted_selector.rs`) now runs `normalize_weights` before `generate_smooth_sequence`, so the precomputed `Box<[u16]>` sequence can no longer be O(sum of weights). `BackendRef.weight` is `Option<i32>` (up to `i32::MAX`) with no upstream clamp, and `selector.rs::create_state` converted it with `w as usize` + `w > 0` filter only; two backends near `i32::MAX` would allocate a ~4.29e9-entry `Vec<u16>` (~8.6 GB) on the first request after route (re)load → OOM-kill, reachable via a routine HTTPRoute/GRPCRoute RBAC edit. `normalize_weights` (1) reduces by GCD — lossless, `[N,N]`→`[1,1]`, `[100,200]`→`[1,2]` — then (2) if the total still exceeds `MAX_SMOOTH_SEQUENCE_LEN = 10_000`, scales every positive weight down proportionally with `.max(1)` so no backend drops out. Ratio (the only thing WRR fairness depends on) is preserved; resolution bounded to 1/10,000; worst-case sequence ~20 KB. Fixed at the constructor (origin) not `selector.rs:91` (symptom), so `with_equal_weights` (webhook targets) and all future callers inherit it; the `select()` hot path is untouched (normalization runs only at construction). Chose proportional scaling over switching to an Envoy-style EDF scheduler because EDF needs a mutable heap (lock per pick) and would abandon this module's deliberate lock-free O(1) design. Unit tests in `weighted_selector.rs`: `gcd_basic`, `normalize_*` (gcd reduction / huge-equal-collapse / coprime-scale-within-budget / zero-preserve), `new_bounds_sequence_for_coprime_huge_weights`, `new_huge_equal_weights_still_distribute_evenly`. Doc: `http-route/lb-algorithms.md` weight-as-ratio note. Re-reports of the O(sum-of-weights) allocation / unclamped weight form are not accepted; verify `WeightedRoundRobin::new` calls `normalize_weights` before re-reporting.

## Resolution references

- `lb-F005` — 2026-05-24.
