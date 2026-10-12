---
name: wrr-new-result-api
description: Use when reviewing WeightedRoundRobin::new / with_equal_weights findings claiming the construction-time assert! calls panic on invalid input, or proposing to simply delete the asserts; new now returns Result<_, WrrError> (fail-close), and removing the guards is rejected because it relocates the panic to the select() hot path.
knowledge_type: regression-guards
---

# lb-FB01 — `WeightedRoundRobin::new` returns `Result`, not panic — FIXED

**Status:** ✅ fixed · 2026-05-25 · code change
**Historical severity:** Low (BadLogic / fail-fast vs graceful-degrade). The original dormant-path assessment is superseded by current evidence that BackendSelector can reach TooManyItems.

## Conclusion

`WeightedRoundRobin::new` and `with_equal_weights`
(`edgion-gateway/src/lb/backend_selector/weighted_selector.rs`) previously enforced four
construction invariants with unconditional `assert!` (real `assert!`, **not** compiled out in
release): non-empty items, `items.len() == weights.len()`, `items.len() <= u16::MAX`, and
`total_weight > 0`. They now return `Result<Self, WrrError>` where `WrrError` is a four-variant
enum (`EmptyItems` / `LengthMismatch` / `TooManyItems` / `ZeroTotalWeight`, `impl Display +
Error`). The BackendRef selection owner degrades gracefully (fail-close):

- `BackendSelector::create_state` (`selector.rs`) maps any `Err` to `SelectorState::Error(ERR_WRR_BUILD_FAILED)`
  (new code `3`), so `select()` returns `Err(code)` and the data plane serves an error response
  instead of panicking. Construction runs during route-generation compilation; selection
  reads the captured state. No request-time initialization or tracing is introduced.
- The webhook path (`link_sys/providers/webhook/manager.rs`) has been migrated (2026-06-20
  endpoint-decouple refactor) and **no longer uses WRR**. `WebhookManager::upsert` now constructs
  `WebhookResolvedTarget::Service { service_key, port, scheme, cursor: AtomicUsize }` for
  `WebhookTarget::Service` entries; `pick_url()` reads ready endpoints directly from the shared
  `EpSliceStore` using a local `AtomicUsize` cursor for round-robin. `Uri` and `ServiceClusterIp`
  targets map to `WebhookResolvedTarget::Static(String)`. The `Pending` and `Endpoints(WRR)`
  variants have been removed. This bullet is preserved for historical reference only.

`BackendSelector::create_state` pre-validates emptiness, lengths and positive weights,
but does not pre-validate WRR's item-count limit. More than 65535 positive backend slots
reaches `WrrError::TooManyItems` and is captured as `ERR_WRR_BUILD_FAILED`. The previous
claim that every WRR error was unreachable was too broad. The selector regression
`selector_weighted_constructor_failure_is_selection_error` checks this mapping without
constructing a smooth sequence. The historical webhook path above is not a current WRR caller.

Re-reports of the form "`WeightedRoundRobin::new` panics via `assert!` on invalid input" are **not
accepted** (fixed); verify the current `new` returns `Result<_, WrrError>` before re-reporting.

## Core Rationale

**1. Aligns with the project's "graceful-degrade on long-running paths, no data-plane panic" rule.**
`new` runs during route-generation construction. A `Result` lets
BackendSelector capture ERR_WRR_BUILD_FAILED instead of aborting construction on invalid input.

**2. Constructor validation preserves valid selection and fails closed on invalid inputs.**
For valid inputs the selection behavior is preserved; invalid-input arms return `Err`
instead of panicking. Item-count failure remains reachable through BackendSelector.

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
- `edgion-gateway/src/link_sys/providers/webhook/manager.rs` — migrated away from WRR (2026-06-20 endpoint-decouple refactor); `Pending` and `Endpoints(WRR)` variants removed
- Sibling LB hardening: `weighted_selector.rs::normalize_weights` (lb-F005 weight-cap, orthogonal OOM guard)

## Current ownership boundary

ARCH-016 removes BackendSelector's obsolete initialization/swap lifecycle. Its constructor
captures immutable state and preserves all error codes, including TooManyItems mapping.
See [BackendRef selector generation ownership](backend-ref-selector-generation-ownership.md).
