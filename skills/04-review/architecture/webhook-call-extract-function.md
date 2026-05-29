---
name: webhook-call-extract-function
description: Use when reviewing link_sys webhook call.rs "webhook_call retry-loop Ok arm mixes four concerns / extract execute_attempt + handle_ok_response / consolidate the scattered record_passive_result calls (F-015)" findings. The Ok-arm extraction is a deliberately declined readability-only refactor.
---

# LinkSys webhook `webhook_call` Ok-arm — Extract-Function — DESIGN-TRADEOFF

**Audit date**: 2026-05-25
**Linked issue**: link-sys F-015 (extract-function / maintainability) — closed, not refactored.

## Conclusion

`webhook_call` (`src/core/gateway/link_sys/providers/webhook/call.rs`, ~139 LOC) has a
retry loop whose `Ok(resp)` arm (lines ~129–183) performs retry-on-status gating,
passive health tracking, success classification, and size-capped body read in one block.
The finding itself states **"No runtime bug exists in the current code"** — this is a
readability-only concern. The proposed extraction into `execute_attempt(...)` and
`handle_ok_response(...) -> ControlFlow<Result<...>>` is **not accepted**.

Fix suggestions of the form "extract `execute_attempt` / `handle_ok_response` so the loop
shrinks to gate + orchestration" or "make `handle_ok_response` the single owner of every
`record_passive_result` call" are **not accepted**.

## Core Rationale

**1. No runtime bug — purely hypothetical future-omission risk**

The issue's Impact section concedes there is no defect today. The argued risk is that a
future developer adds a new response branch (redirect-follow, 204 fast path) and forgets a
health-tracking call. That is a speculative maintenance concern, not an observable bug, and
the project rule in [style-consolidation-thresholds.md](style-consolidation-thresholds.md)
requires "real maintenance cost or reading cost evidence" before acting on a "looks
like it mixes concerns" finding.

**2. The "single owner of all `record_passive_result`" claim is overstated**

There are **four** `record_passive_result` call sites in `webhook_call`: the
success/graded result (line ~151), the body-read error (line ~173), the connect/send
error arm (line ~190), and the post-loop exhaustion (line ~202). The proposed
`handle_ok_response` would absorb only the two inside the `Ok` arm (151, 173); the send-Err
arm (190) and the exhaustion path (202) stay in the loop body. So the extraction does not
eliminate the "add a branch and forget the call" risk — it relocates two of four sites
while leaving the other two outside. The marketed safety property is not actually achieved.

**3. Mechanical control-flow extraction is exactly where regressions sneak in**

The Ok arm interleaves `continue` (retry-on-status, gated by `attempt + 1 < max_attempts`),
two early `return Err(...)`, and a final `return Ok(...)`. Re-encoding that behind a
`ControlFlow<Result<...>>` return plus two helpers carrying 6–7 parameters each and a
`&mut String last_error` is meaningful machinery for a marginal readability gain — and the
retry/return boundary semantics are precisely the kind a mechanical collapse can silently
regress. The loop is already covered by unit tests (`successful_2xx_call`,
`status_not_success`, `response_too_large`, `retry_on_status_retries_and_succeeds`, etc.);
restructuring it trades a real regression surface for no behavior change.

**4. Network-I/O-bound, not a perf concern either**

Each iteration awaits `builder.send()`, so function-call overhead is irrelevant; the
extraction has no performance dimension. The only axis is readability, which falls below
the project threshold given the cost and risk. Same refactor-decline posture as the sibling
[link-sys-dispatch-extract-function.md](link-sys-dispatch-extract-function.md),
[plugin-runtime-god-match.md](plugin-runtime-god-match.md), and
[style-consolidation-thresholds.md](style-consolidation-thresholds.md).

## Fix Suggestions Not Accepted

- "Extract `execute_attempt` (pure dispatch) + `handle_ok_response` (classify + track + read body) returning `ControlFlow`" — re-encodes a tested continue/return control flow behind multi-param helpers for readability-only gain; regression risk out of proportion.
- "`handle_ok_response` becomes the single owner of all `record_passive_result` calls" — false: the send-Err (line ~190) and exhaustion (line ~202) sites stay outside the helper, so the omission risk is relocated, not removed.
- "The Ok arm mixes four concerns and must be split" — concern-mixing alone is not a defect; the four steps run in a fixed linear order with no branching reuse, so splitting adds indirection without removing duplication.

## Re-evaluation Triggers

Re-open only if:

- A genuinely new `Ok`-arm response branch is added (e.g. redirect-follow, 204-no-body fast path) that needs its own distinct health-tracking — extract *at that point*, together with the new branch, where the duplication becomes concrete.
- A real bug is traced to a `record_passive_result` call site that was added/changed in one path but forgotten in another.
- The `Ok` arm grows substantially (well beyond its current ~55 LOC) such that reading cost becomes a measured, not speculative, problem.

## Reference Cases

- link-sys F-015 (`tasks/link/link-sys-F015-webhook-call-rs-extract-function.md`), audited 2026-05-25.
- `src/core/gateway/link_sys/providers/webhook/call.rs` — `webhook_call`, the four `record_passive_result` sites.
- Sibling refactor-decline decisions: [link-sys-dispatch-extract-function.md](link-sys-dispatch-extract-function.md), [plugin-runtime-god-match.md](plugin-runtime-god-match.md), [style-consolidation-thresholds.md](style-consolidation-thresholds.md), [size-and-similarity-heuristics.md](size-and-similarity-heuristics.md).
