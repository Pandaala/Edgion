# Add Informational-Response Per-Try Timeout Retry Integration Coverage

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | bugfix |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Add an HTTP/1 integration scenario proving that a non-terminal informational response does not suppress a configured backend-request timeout retry.

## Requirements

- Extend the established per-try timeout harness after the campaign's frozen-test period.
- Make the first upstream attempt send `100 Continue` and then stall beyond `backendRequest`; make the second attempt succeed.
- Assert that the retry reaches the successful attempt, the first upstream attempt is diagnosed as `504` with `BackendRequestTimeoutRetry`, and no duplicate or corrupted final response head reaches the client.
- Use bounded waits and deterministic cleanup.

## Scope

### In scope

- Existing HTTP retry/per-try integration infrastructure after the freeze is lifted.
- Minimal upstream test-server support needed to emit an informational head and then stall.

### Out of scope

- Editing an existing `edgion-tests/**` file during the quality campaign.
- Changing production retry counts, budgets, buffering, status mapping, or access-log ownership.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-09 | Defer from B0180 | The meaningful end-to-end scenario requires extending frozen integration infrastructure; local H1 session tests cover the production fix now. |
