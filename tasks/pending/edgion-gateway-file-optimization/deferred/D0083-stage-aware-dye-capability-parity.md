# D0083 — Define Stage-aware Dye Capability and Producer Parity

## Meta

- Status: needs-discussion
- Severity: medium
- Discovered in: B0122 runtime dye plan analysis
- Category: plugin-runtime-contract
- Canonical task: [`tasks/todo/define-stage-aware-dye-capability-and-producer-parity.md`](../../../todo/define-stage-aware-dye-capability-and-producer-parity.md)

## Evidence

Dye validation currently uses plugin-type-wide capabilities even though actual outcome production depends on the runtime stage and execution path:

- Wasm can carry fail-open/fail-close dye in the response-body stage, but its per-chunk callback leaves the received session unused and records no outcome. Response headers are already committed at that point, so any response operation first queued there would also miss the normal upstream-header drain point and could not render through the current lifecycle.
- AiProxy records fail-open, fail-close, and reject outcomes while declaring no public dye capabilities or prefix.
- ExtProc streamed/trailer paths record success while the declared capability table exposes only failure outcomes.

The source and user documentation also link to `docs/superpowers/specs/2026-07-20-entry-level-dye-suite-design.md`, which is absent from the current tree and history.

## Why deferred

The correct result requires a product-level decision for each plugin/stage pair: reject unsupported configuration, expose an intentional capability, remove vestigial producers, or redesign around response-header commit timing. A local `dye_plan.rs` representation edit cannot make that decision safely, especially for streaming response-body paths where fail-close and header mutation may no longer be possible.

## Integration-test impact

No integration change is required to record or design this contract. Crate-local validation/publication parity tests should pin supported combinations first. If the final design exposes any previously unsupported public dye behavior, that future feature must create a separate integration-coverage issue rather than modify existing integration tests during this campaign.
