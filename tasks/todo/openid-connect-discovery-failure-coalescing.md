# Coalesce and Back Off Failed OpenID Connect Discovery Fetches

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | reliability |
| Priority | P2 |
| Issue | N/A |
| Depends on | B0092 |

## AI guide summary

Give concurrent cold OpenID Connect discovery callers one cancellation-safe in-flight outcome and a short bounded failure backoff so a provider outage cannot turn request concurrency into sequential provider timeouts.

## Requirements

- Represent one shared in-flight discovery outcome per provider generation; concurrent cold callers must not start independent fetches.
- Ensure cancellation of one waiter, including the initiating request, does not unexpectedly cancel the shared fetch needed by remaining waiters.
- Publish a short bounded failure state or backoff deadline without permanently poisoning the provider; retry after the deadline and atomically publish the recovered immutable discovery snapshot.
- Define interactions among constructor prewarm, lazy first use, ordinary TTL refresh, stale fallback, forced JWKS refresh, and generation replacement.
- Preserve response-size limits, discovery validation, timeout policy, Dye behavior, and the existing success-cache state machine.
- Coordinate public error redaction with D0058. Do not expose or log endpoints, addresses, provider bodies, parser text, tokens, credentials, or Secret metadata.
- Keep dial-time destination enforcement in D0056; this task must not invent a separate egress path or client.
- Keep request-level observability in the canonical access log; do not add tracing or per-request event logs.

## Acceptance criteria

- N concurrent cold callers observe one outbound discovery attempt and one typed outcome.
- Cancelling the initiating caller or another waiter does not strand or duplicate the shared attempt.
- Repeated calls during the failure backoff do not contact the provider; a call after the deadline retries and successful recovery publishes one new `Arc` snapshot.
- Provider transport, HTTP, size-cap, and validator failures cannot permanently poison the cache.
- Tests cover concurrent failure, leader and waiter cancellation, bounded backoff, retry, recovery, and generation replacement using provider-local deterministic fixtures.
- Existing integration tests remain unchanged unless a later approved design introduces behavior that cannot be verified locally.
- Workspace format/check/Clippy and repository guards pass.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-01 | Defer failure coalescing from B0092. | Sharing successful immutable snapshots removes deep clones but does not define safe ownership, backoff, or recovery for failed discovery attempts. |
