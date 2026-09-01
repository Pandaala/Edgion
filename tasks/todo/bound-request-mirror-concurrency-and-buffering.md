# Bound RequestMirror Concurrency and Buffering

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | correctness |
| Priority | P1 |
| Issue | N/A |
| Depends on | B0102 |

## AI guide summary

Define and enforce one safe numeric domain for RequestMirror concurrency and body-tee buffering before either value can reach Tokio allocation primitives.

## Requirements

- Define explicit upper bounds for `maxConcurrent` and `maxBufferedChunks` from Tokio limits and the Gateway memory budget.
- Apply one canonical validation contract to HTTPRoute annotations and EdgionPlugins resources, with Controller/Gateway preparse parity.
- Keep a defensive Gateway construction boundary so direct in-process construction cannot panic.
- Preserve bounded request-body tee, semaphore rejection, cancellation, access-log, and main-request `GoodNext` semantics.
- Update English and Chinese documentation together.
- Keep existing integration tests frozen; create a separate integration issue only for a genuine wire-visible gap.

## Acceptance criteria

- No accepted value can panic semaphore or channel construction.
- Resource, Controller, Gateway, and annotation paths agree on the valid numeric domain.
- Deterministic boundary tests cover zero/default/minimum/maximum/overflow values.
- Workspace format/check/Clippy and repository guards pass.
