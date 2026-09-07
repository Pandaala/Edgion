---
name: build-lock-single-mutex-intentional
description: Use when reviewing ServiceSelectorCell concurrency findings claiming the single build_lock serializes first-build across distinct policy slots; the single coarse mutex is an intentional simplicity/correctness tradeoff and per-slot mutexes are not accepted.
knowledge_type: intentional-design
---

# lb-F010 — single `build_lock` covering all four policy slots is intentional — DESIGN-TRADEOFF

**Status:** ⚠️ accepted tradeoff · 2026-05-24 · no code change
**Severity:** Low (BadLogic) — warm-up-only, rarely-triggered latency, no correctness impact

## Conclusion

`ServiceSelectorCell` (`edgion-gateway/src/lb/compiled_selector.rs`) deliberately uses one
`build_lock: Mutex<()>` to cover the "compile a slot" critical section for all four policies
(RR / CH / LeastConn / EWMA). Two concurrent first-build requests for *different* policies do
serialize behind it, but this contention is init-only, bounded by `O(N_policies × build_time)`,
and almost never hit in practice. The single lock buys a clean three-way mutual-exclusion
invariant between `get_or_build`, `replace_backends`, and `hydrate_if_empty` that the proposed
per-slot scheme would replace with a multi-lock ordering protocol.

Fix suggestions of the form "replace `build_lock` with `[Mutex<()>; 4]` indexed by
`PolicyKind as usize`" or "use per-slot `OnceLock` + a coarse lock only for `replace_backends`"
are **not accepted**.

## Core Rationale

**1. The contention is init-only and rarely triggered.**

The slow path (`get_or_build`, `compiled_selector.rs`) takes the lock only on the *first* build
per `(service, policy)`. Steady-state requests hit the lock-free `ArcSwap` fast path
(`slot.load_full()`) and never touch the mutex. The issue itself notes most services use a
single policy, so the cross-policy serialization window requires the rare combination of one
service serving multiple distinct LB policies *simultaneously at warm-up*. Of the four builds,
only `RrCore::build` and `ChCore::build` (ketama ring) have meaningful cost; `MetricCore::build`
(LeastConn / EWMA) just wraps the `Arc`, so the realistic worst case is one extra RR/CH build
serialized once.

**2. The single lock is the correctness invariant, not incidental.**

`replace_backends` holds `build_lock` while it both swaps the backend snapshot *and* clears all
four slots, so a concurrent `get_or_build` always observes either the pre-replace state (old
backends + old slots) or the post-replace state (new backends + empty slots) — never the torn
"new backends + stale addr_map slot" state that would emit spurious upstream-not-found errors
(documented at `replace_backends`'s doc comment). `hydrate_if_empty` shares the same lock so its
empty-check-then-store is atomic against `replace_backends`. One mutex gives all three paths
mutual exclusion for free.

**3. Per-slot mutexes trade a one-time warm-up win for permanent concurrency complexity.**

To keep invariant (2), the per-slot scheme forces `replace_backends` *and* `hydrate_if_empty` to
acquire all four slot locks simultaneously, in a fixed index order, to avoid deadlock — turning a
single atomic critical section into a lock-ordering protocol every future maintainer must
preserve. This is a medium-risk concurrency change (the highest-risk class to get subtly wrong)
in exchange for shaving a rarely-hit, bounded warm-up latency. The cost/risk is out of
proportion to the benefit. This matches the project's standing posture on LB cold-path
micro-optimizations (`concurrency/lb-false-positives.md`, `cpu-memory/` cases): do not add
concurrency complexity to a cold/one-shot path.

## Fix Suggestions Not Accepted

- `[Mutex<()>; 4]` per-slot mutexes — requires `replace_backends` + `hydrate_if_empty` to hold
  all four locks in index order (deadlock-avoidance invariant); large concurrency-risk increase
  for an init-only, rarely-triggered latency win.
- per-slot `OnceLock` + coarse lock for `replace_backends` only — resetting a `OnceLock` on
  `replace_backends` still needs exclusivity against in-flight builds, so it reintroduces the
  same coordination it claims to remove, plus a second locking mechanism.

## Re-evaluation Triggers

Re-open this decision only if:

- A profiling trace shows selector build serialization as a real warm-up bottleneck (e.g. very
  large backend sets compiling multiple CH rings concurrently on a hot restart).
- `ServiceSelectorCell` gains a per-policy build path whose build cost becomes non-trivial and
  routinely concurrent (not just RR/CH at first request).
- The slot set grows well beyond four, making a single coarse lock a measurable warm-up serializer.

## Reference Cases

- lb-F010 (review finding, 2026-05-24)
- `edgion-gateway/src/lb/compiled_selector.rs` — `ServiceSelectorCell::build_lock`,
  `get_or_build`, `replace_backends`, `hydrate_if_empty`
- Standing posture: `concurrency/lb-false-positives.md`, `cpu-memory/performance-review-methodology.md`

## Resolution references

- `lb-F010` — 2026-05-24 (⚠️ accepted tradeoff).
