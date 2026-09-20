# B0056 — AI Connect Failure Scope

## Scope

- Baseline: `cb7d2a5323f48d9a96e32751614e2197543f083b`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Owned code file: `edgion-gateway/src/routes/http/proxy_http/pg_fail_to_connect.rs`.
- Allowed knowledge file: `skills/04-review/ai-routing/regression-guards/ai-endpoint-failure-skips-the-service.md`.
- Existing `edgion-tests/**` files, unrelated runtime/knowledge files, and protected user paths remain forbidden.
- Exact commit allow-list: the two technical files above, any independently verified B0056 deferred-task/index records, and B0055/B0056 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Analysis verdict: `change`; implementation uses plan-first because it touches retry and error lifecycle semantics.
- AI TCP/TLS connection establishment fails before provider credentials are dispatched, and the selected EdgionBackend endpoint is shared by all credentials for that service.
- The current retry-eligible branch uses credential-scoped predispatch advancement, so a dead endpoint can be retried once per credential instead of being skipped service-wide.
- Use the canonical endpoint-scoped `AiPeerPredispatchFailure::Endpoint.finish(ctx)` operation to release the reservation and advance past the failed service exactly once.
- Preserve the relay-disabled settle-only branch, deadline ordering, attempt/outlier accounting, retry count and budget semantics, response classification, and all observability behavior.

## Plan requirements

1. Keep relay-disabled handling as reservation release only, with no retry-owned scheduler effect.
2. In the retry-eligible branch, use endpoint-scoped predispatch failure and set Pingora retry only when service-wide successor selection succeeds.
3. Add focused in-file coverage for service-wide skip and existing relay-state behavior without modifying frozen integration tests.
4. Update the approved regression guard to explicitly classify TCP/TLS connection establishment as endpoint-scoped predispatch failure.

## Verification plan

- Focused AI dispatch and `pg_fail_to_connect` tests plus nearby proxy retry tests.
- Full campaign repository gates and frozen-path verification.

## Approved plan and implementation

- An independent plan reviewer approved the explicit settle-only versus endpoint-settle-and-skip design and required multi-credential/multi-service assertions.
- Retry-eligible AI TCP/TLS connect failures now call `AiPeerPredispatchFailure::Endpoint.finish(ctx)` exactly once and mark Pingora retry only after successful service-wide advancement.
- Relay-disabled handling still releases the active reservation without scheduler advancement and remains non-retryable.
- Focused tests prove the failed service's second credential is not scanned, the next service remains selectable, dispatch/failure-ledger counts remain zero, failed advancement is non-retryable, and relay veto preserves the current scheduler target.
- The regression guard now classifies connection establishment as endpoint-scoped and documents next-scan selection and exactly-once settlement.

## Independent code review

- A fresh final reviewer approved the implementation with no Must Fix or Should Fix.
- The review verified endpoint ownership, exactly-once reservation release, scheduler/service exclusion, retry flag gating, deadline/relay ordering, attempt/outlier preservation, zero AI budget/ledger effects, and scope containment.
- The reviewer independently verified D0009 and D0010 as separate cross-cutting tasks.

## Verification

- Focused tests passed: 9 `pg_fail_to_connect` tests and the canonical endpoint-predispatch regression test.
- Full repository gates passed: formatting, workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only.
- Only the campaign's known Kafka and frozen integration-test clippy warnings plus cargo-deny duplicate/advisory-ignore warnings remain.
- Final frozen-path verification passed for all 3,442 recorded integration paths.

## Deferred findings

- D0009: ordinary route retry configuration can exceed the locked Pingora proxy loop ceiling.
- D0010: AI connect exhaustion at that ceiling can bypass the next-scan OpenAI error renderer and fall through to generic HTML 503.

## Handoff

- Ledger/changelog and D0009/D0010 records updated: yes.
- Commit: `535af96d2f790c6596060cc1bcfef00998c0e076`.

## Protected paths

- `task/`
- `tasks/pingora-0.9.0/`
- `tasks/todo/complete-k8s-dynamic-scenario-parity/`
- `waf-architecture-and-multi-pattern-matching.md`
