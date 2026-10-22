# Evaluate native Messages iteration-model storage

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `dbd56c9e0`. No runtime or attribution-policy change.

## Evidence

`Selector::on_captured_string` in
`edgion-gateway/src/ai/protocols/anthropic/usage.rs` copies each selected iteration
model into `current_iteration_model`. At object close it compares that value with
`iteration_model`, then moves the current String into the previous-model slot.
Repeated nonempty equal model names therefore still incur owned string creation
and copying. Empty strings need not allocate. This is not an accumulated list:
the selector retains current/previous iteration models and the separate top-level
model, rather than one String per completed item.

Model capture has a 512 decoded-byte cap. The iteration counter rejects beyond
4096 container items in the current selected iteration context; selected parent
replacement resets that counter, and scalar items do not increment it. Do not
treat it as a universal per-document item cap. Independent source, event, depth,
decoded, retained and work limits remain active. Actual iteration frequency and
model lengths determine cost; no allocation profile or throughput result exists.

The callback's text borrows canonical parser scratch only synchronously. Model
attribution must survive later chunks and finalization. The top-level model may
appear after iterations; `Observer::finish` compares against that final model or
the supplied expected model. The SSE processor separately preserves the actual
start-model attribution. Borrowing callback text across these boundaries is not
a valid optimization.

## Decision and acceptance

Measure before changing representation. Options are keeping current owned values,
representing an explicitly supplied model equal to the previous model with a
separate state, or reusing owned scratch buffers. A same-model marker adds state
transitions; buffer reuse retains capacity that current replacement can release.
Compare allocation calls/bytes, copied bytes, live/peak capacity and full observer
CPU across absent, one, repeated-equal, alternating and maximum-length models.
Include actual JSON/SSE chunk schedules and malformed or duplicate fields. Do not
infer request speedup from a synthetic repeated-model case alone.

Every iteration must supply its own valid final model and message type. Never
let a previous item's value fill an omitted field. Distinguish missing, malformed
and explicitly equal models. Preserve same-item last-wins duplicates, valid
replacement of invalid values, selected usage/message/iterations parent resets,
composite-state behavior and the final actual/expected-model comparison. A model
equal to the previous item is not automatically equal to the final top-level
model. Do not introduce hash-only equality, model interning, a global cache or
another response-body store.

Acceptance must compare observation fields and accounting outcomes over every
relevant split, including valid-to-invalid and invalid-to-valid duplicate models,
missing models on either side of a valid item, mixed item types, late top-level
models and repeated parent replacement. Keep all existing parser limits and
failure behavior, raw response bytes, reported-only accounting, cumulative totals
and native lifecycle ownership. Same-model iteration summaries must not be added
again as extra usage.

Original authority:
`skills/04-review/ai-routing/regression-guards/native-messages-preservation-and-accounting.md`.
This task is separate from usage-01 character counting and usage-02 accounting
snapshot timing. Implementation requires a separate IMPL plan, exact source
allow-list and independent review; this task approves no representation change.

Fresh unchanged-code verification passed 35 Anthropic protocol tests, formatting,
workspace all-target check/Clippy and agent-doc validation. Existing unrelated
warnings remain. Tests establish current behavior, not proposed storage equivalence.
No new tests, full suite, live provider, integration or benchmark ran.
