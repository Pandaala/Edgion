# D0010 — AI Connect Ceiling Error Rendering

## Meta

- Status: resolved-with-D0009
- Severity: medium
- Discovered in: B0056, `edgion-gateway/src/routes/http/proxy_http/pg_fail_to_connect.rs`
- Category: AI terminal error contract

## Evidence

Normal AI successor exhaustion re-enters `upstream_peer` and uses the
OpenAI-compatible `BackendUnavailable` JSON renderer. With 16 endpoint-scoped
connect failures, however, the final callback can advance the scheduler and mark
the error retryable after Pingora has consumed its 16-loop ceiling. Pingora then
exits without the next `upstream_peer` scan and generic `fail_to_proxy` can emit
an HTML 503 instead of the AI error envelope.

## Why deferred

The correct fix crosses synchronous connect-failure handling, Pingora loop and
terminal ownership, scheduler exhaustion, and the asynchronous OpenAI response
renderer. It also interacts directly with D0009's retry-ceiling contract. A
local response from `fail_to_connect` would duplicate terminal ownership and is
not safe to add within B0056.

## Resolution

- `skills/04-review/routing/regression-guards/retry-budget-owns-pingora-loop-capacity.md`

The Pingora loop is now capacity for the full public route retry domain instead
of an independent 16-iteration limit. AI remains bounded to 16 candidate
backends and 8 dispatched attempts, both below that capacity, so exhaustion
renders its own terminal response before the Pingora boundary can intervene.
