# Refresh AllEndpointStatus Outbound-header Review Records

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | docs |
| Priority | P3 |
| Issue | N/A |

## AI guide summary

Correct closed-review and user-documentation statements that still describe obsolete AllEndpointStatus outbound-header and summary behavior. The runtime already filters hop-owned body-framing and expectation headers and counts only 2xx/3xx probe responses as success.

## Requirements

- Update `skills/04-review/CLOSED-FINDINGS.md` entry `FA-BODY-002` so it no longer says AllEndpointStatus relays `Expect` or `Content-Length`, and remove or replace the nonexistent follow-up task reference.
- Update `skills/04-review/forward-auth/outbound-expect-continue-not-relayed.md` to describe AllEndpointStatus as an aligned independent-builder consumer, not an adjacent gap.
- Update `skills/04-review/http1/hop-by-hop-dynamic-stripping.md` so its AllEndpointStatus/header inventory matches the current implementation.
- Find the AllEndpointStatus user-facing summary text and define `failure_count` as responses outside 2xx/3xx plus transport/time-limit failures, matching runtime behavior.
- Verify every statement against current source and existing focused tests; do not alter runtime code merely to preserve stale prose.

## Scope

### In scope

- The three named review-knowledge records.
- Directly related English and Chinese AllEndpointStatus user documentation if both copies contain the stale summary definition.
- Link repair for the missing follow-up reference.

### Out of scope

- Runtime header filtering, fan-out, or response-schema changes.
- Reopening the fixed ForwardAuth findings.
- New integration tests or changes to existing integration scenarios.

## Acceptance criteria

- No maintained review record claims AllEndpointStatus forwards `Content-Length`, `Transfer-Encoding`, or `Expect` from the inbound request.
- `FA-BODY-002` remains correctly scoped to ForwardAuth's expectation-header fix.
- AllEndpointStatus user documentation defines success as 2xx/3xx and failure consistently with the runtime summary.
- All repaired links resolve, bilingual documentation remains aligned where applicable, and agent-doc validation passes.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-01 | Defer stale review and user-doc corrections from B0063. | They are real knowledge debt but not part of the runtime security/correctness implementation boundary. |
