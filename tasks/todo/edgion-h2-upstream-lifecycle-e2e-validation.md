# Validate upstream H2 lifecycle behavior through Edgion

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-25 |
| Status | pending |
| Type | test coverage |
| Priority | P2 |
| Origin | Edgion verification follow-up split from H2-012 |

## AI guide summary

Audit and complete real-Gateway integration coverage for the existing Pingora
fork's upstream H2 lifecycle protections. Reuse current transport scenarios and
add only missing deterministic cases. This is an independent verification task:
it does not require simplifying the END_STREAM watcher or waiting for an upstream
h2 trailer-decoder fix. Missing coverage is not evidence of a production defect.

## Current evidence and limits

Source inspection on 2026-09-25 established:

- Edgion already uses the `edgion_v5` fork branch, `boring`/`boring-sys` 5, and
  locked `h2 0.4.19`. The former dependency-migration blockers are obsolete.
- `edgion-tests/src/client/suites/http_route/transport/transport.rs` registers
  `upstream_reset_no_error_after_complete_response`. Its implementation,
  `shape_upstream_reset_after_response`, asserts full downstream delivery and
  subsequent reuse of the same upstream connection.
- `edgion-tests/src/server/transport/h2/wire_proof.rs` contains frame-level
  backend proofs for complete-response-then-reset ordering. These backend tests
  complement, but do not replace, running the scenario through a real Gateway.
- Transport recorder events already expose connection identity. Do not add a
  second observability mechanism merely to identify a reused connection.
- Early-response scenarios exist, but their presence alone does not prove the
  specific flow-control capacity-stall contract below.

No integration tests were run while creating this task. Existing test code is a
starting point for verification, not a fresh passing result. The remaining rows
need a coverage audit before being classified as missing tests.

## Acceptance matrix

| Contract | Required evidence | Initial status |
|----------|-------------------|----------------|
| Complete response followed by NO_ERROR reset | While the client is still uploading, prove the upstream emits complete response then reset; the client receives the full response, and a follow-up request reuses the upstream connection | Scenario exists; rerun and review assertions |
| H2-005: persistent GOAWAY ceiling and fail-closed state | Controlled GOAWAY sequences cannot relax a previously lowered stream ceiling or restore eligibility after poisoning; observe permitted in-flight outcomes and connection selection through the Gateway | Audit coverage, then add missing cases |
| H2-007: bounded upload after peer EOS | Deterministically exhaust upstream request-body flow-control capacity, complete the upstream response, and prove the blocked upload exits within a bounded interval without losing response bytes | Audit coverage; ordinary early-response coverage is insufficient |
| H2-008: shutdown connection eligibility | After shutdown is observed, no new request is allocated to the connection; a follow-up uses another eligible connection while already accepted streams obey the recorded contract | Audit coverage, then add missing cases |

## Scope and boundaries

- Extend `edgion-tests` transport scenarios, backend controls, and fixtures only
  as required by the acceptance matrix. Preserve existing assertions and gates.
- Use controlled protocol events and connection identities to prove the trigger
  and outcome; arbitrary sleeps and a successful status code alone are not proof.
- Keep response handling streaming and test observations bounded.
- Do not add HTTP response caching to Edgion. Cache-admission contracts remain
  in the Pingora fork's own test suite.
- Do not simplify the watcher, vendor/path-patch h2, change dependency versions,
  or change production behavior as part of this verification task. If a test
  exposes a defect, record its reproducer and assess a separate fix scope.
- The downstream body-filter Trailer/Done EOS callback fix is separate and is
  not closed or reimplemented by this task.

## Work and closure

1. Read repository rules, the testing guide, and the relevant H2 knowledge below.
   Revalidate recorded contracts against the actual resolved fork revision.
2. Map each acceptance row to current tests and assertions. Reuse sufficient
   cases; add only the missing deterministic coverage.
3. Run the targeted real-Gateway transport scenarios, plus focused backend tests
   when their controls change. Record commands, Gateway/fork revisions, protocol
   combinations, results, and any unresolved limitations. Do not substitute a
   sibling checkout for the dependency actually used by the Gateway build.
4. Obtain independent review of the trigger evidence and assertions. Close only
   when every row has passing, reproducible evidence or an explicitly approved
   disposition; record any unresolved work separately before removing this task.

## References

- `skills/05-testing/01-integration-testing.md`
- `../pingora/edgion-changes/review/h2-grpc/h2-goaway-persistent-ceiling-fail-closed.md`
- `skills/04-review/h2-grpc/regression-guards/h2-writer-capacity-stall-after-response.md`
- `skills/04-review/h2-grpc/regression-guards/h2-shutdown-connection-not-allocatable.md`
- `skills/04-review/h2-grpc/regression-guards/h2-proxy-cache-reuse-harness-consumers.md`
- `skills/04-review/h2-grpc/regression-guards/h2-trailer-decoder-upstream-limitation.md`
- `tasks/block/pingora-h2-end-stream-watch-simplification.md`

The watcher-simplification task remains separate and deferred under its upstream
decoder policy. Its historical H2-012 fork-harness work motivated this product
verification task; it is not a prerequisite for executing it.
