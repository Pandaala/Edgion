# Define Stage-aware Dye Capability and Producer Parity

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | design-correctness |
| Priority | P2 |
| Issue | N/A |
| Depends on | Canonical entry-level dye validation, immutable plugin publication, and response-header commit timing |

## AI guide summary

Make every accepted plugin/stage/outcome dye combination correspond to a real runtime producer and a response mutation point that can still honor its documented semantics.

## Requirements

- Inventory declared dye capabilities and every `record_outcome` producer by plugin variant, runtime stage, and execution path.
- Define one canonical stage/path-aware capability contract; do not add a second dye model or plugin-local validation mechanism.
- Decide whether Wasm response-body dye must be rejected or explicitly supported without response aggregation, per-chunk logging, or false fail-close claims after headers commit.
- Decide whether AiProxy fail-open/fail-close/reject outcomes become supported public dye behavior or the producers that are currently unreachable to the dye renderer are removed.
- Decide whether ExtProc response success becomes configurable or the producers that are currently unreachable to the dye renderer are removed.
- Preserve active/disabled/policy-stamped behavior, B0121 condition fail-close versus observational unresolved semantics, header sanitization, set/append ordering, redaction, observability restraint, and response streaming.
- Restore the missing dye design authority or replace every stale source and user-documentation link with the actual canonical record.
- Do not modify existing integration-test files.

## Acceptance criteria

- A focused parity guard proves every accepted `(plugin, stage, execution path, effective configuration, outcome)` combination has an intentional producer and every intentional configurable producer is declared.
- Unsupported configurations fail validation consistently in Controller and Gateway paths before activation.
- Response-header commit timing is explicit for every supported response-stage outcome.
- No request or response-body hot path gains unbounded state, new tracing, or parallel header-mutation logic.
- If any previously unsupported public dye behavior is exposed, a separate integration-coverage issue is created and owned by that feature.
