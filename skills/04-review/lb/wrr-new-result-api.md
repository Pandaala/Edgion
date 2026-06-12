---
name: wrr-new-result-api
description: Use when reviewing WeightedRoundRobin::new / with_equal_weights findings claiming the construction-time assert! calls panic on invalid input, or proposing to simply delete the asserts; new now returns Result<_, WrrError> (fail-close), and removing the guards is rejected because it relocates the panic to the select() hot path.
---

# lb-FB01 — `WeightedRoundRobin::new` returns `Result`, not panic — FIXED

**Status:** ✅ fixed · 2026-05-25 · code change
**Severity:** Low (BadLogic / fail-fast vs graceful-degrade) — was dormant (no caller could trigger it)

## Conclusion

`WeightedRoundRobin::new` and `with_equal_weights`
(`edgion-gateway/src/lb/backend_selector/weighted_selector.rs`) previously enforced four
construction invariants with unconditional `assert!` (real `assert!`, **not** compiled out in
release): non-empty items, `items.len() == weights.len()`, `items.len() <= u16::MAX`, and
`total_weight > 0`. They now return `Result<Self, WrrError>` where `WrrError` is a four-variant
enum (`EmptyItems` / `LengthMismatch` / `TooManyItems` / `ZeroTotalWeight`, `impl Display +
Error`). The two callers degrade gracefully (fail-close):

- `BackendSelector::create_state` (`selector.rs`) maps any `Err` to `SelectorState::Error(ERR_WRR_BUILD_FAILED)`
  (new code `3`), so `select()` returns `Err(code)` and the data plane serves an error response
  instead of panicking. No `tracing::` is added here — this path is request-triggered lazy init.
- The webhook resolver (`link_sys/providers/webhook/manager.rs`) matches the `Result`; on `Err`
  it emits `tracing::warn!` (registration time, off the request hot path) and falls back to
  `WebhookResolvedTarget::Pending`.

Both call sites still pre-validate their inputs, so `WrrError` is currently unreachable in
production — the conversion is forward-looking hardening for a future in-crate caller that forgets
to pre-validate.

Re-reports of the form "`WeightedRoundRobin::new` panics via `assert!` on invalid input" are **not
accepted** (fixed); verify the current `new` returns `Result<_, WrrError>` before re-reporting.

## Core Rationale

**1. Aligns with the project's "graceful-degrade on long-running paths, no data-plane panic" rule.**
`new` runs at construction (lazy route init / webhook resolve), a long-lived path. A `Result` lets
the caller fail-close (error code / `Pending`) instead of aborting the task on a future misuse.

**2. The fix is at the origin, the behavior change is confined to unreachable paths.**
For valid inputs the behavior is byte-for-byte identical; only the (currently dead) invalid-input
arms changed from `panic!` to `Err`.

## Fix Suggestions Not Accepted

- **"Just delete the four `assert!`."** Rejected — the asserts guard invariants that the lock-free
  `select()` hot path depends on. Deleting them does **not** remove the panic, it relocates it to
  the per-request path or silently corrupts selection:
  - empty items / zero total weight → empty `sequence` → `select()` computes `idx % sequence.len()`
    = `% 0` → **modulo-by-zero panic on the `#[inline]` hot path**.
  - `items.len() != weights.len()` → sequence indices range over `weights.len()` but are used to
    index `items` → **out-of-bounds panic in `select()`** (or silently unreachable backends).
  - `items.len() > u16::MAX` → `max_idx as u16` truncates → **silently selects the wrong backend**.

  Returning a `Result` from `new` (fail at construction, recover at the caller) is strictly better
  than both the old assert and the "delete it" proposal.

## Re-evaluation Triggers

- A new in-crate caller of `WeightedRoundRobin::new` / `with_equal_weights` is added — confirm it
  handles the `Result` and fails closed (does not `.unwrap()` on a data-plane path).

## Reference Cases

- lb-FB01 (review finding, promoted to task then fixed 2026-05-25)
- `edgion-gateway/src/lb/backend_selector/weighted_selector.rs` — `WrrError`, `new`, `with_equal_weights`
- `edgion-gateway/src/lb/backend_selector/selector.rs` — `ERR_WRR_BUILD_FAILED`, `create_state`
- `edgion-gateway/src/link_sys/providers/webhook/manager.rs` — `Pending` fallback
- Sibling LB hardening: `weighted_selector.rs::normalize_weights` (lb-F005 weight-cap, orthogonal OOM guard)
