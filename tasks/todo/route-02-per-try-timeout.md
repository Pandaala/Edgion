---
status: todo
origin: tasks/review-envoy/route-02-no-per-try-timeout.md (promoted via fix-review-issue scope gate)
module: routing
severity: high
type: feature
envoy_reference: Envoy route.RetryPolicy.per_try_timeout — independent timeout per individual upstream attempt
---

# Add per-try timeout to HTTPRoute retry policy

Promoted from a `review-envoy` finding because it is a feature-level change
(new public CRD field + parsing layer + data-plane hot-path semantics), not a
narrow review fix. Handle via the brainstorming workflow before implementation.

## Problem (the gap)

Edgion exposes only two retry/timeout dimensions:
- `HTTPRouteTimeouts.request` — end-to-end deadline (`request_timeout`).
- `HTTPRouteTimeouts.backendRequest` — per-attempt I/O timeout.

`HTTPRouteRetry` (`src/types/resources/http_route.rs:746-768`) has only
`attempts`, `backoff`, `codes`; `ParsedRouteRetry` (`http_route.rs:849-856`)
mirrors this — there is no per-try timeout.

At runtime, `configure_peer_timeouts`
(`src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs:430-499`) folds
three upper bounds via `min(effective_backend_timeout, request_remaining,
grpc_remaining)`. `request_remaining` shrinks with elapsed wall-clock from
`ctx.start_time`, so a slow-but-not-dead first attempt can consume the whole
`request_timeout` budget, leaving zero room for any configured retry.

Example: `request: 10s`, `backendRequest: 9s`, `attempts: 3`. A first backend
that takes 9.5s to return a 503 fires the end-to-end deadline; none of the
retries execute. The configured retry count is misleading to operators.

## Envoy baseline

Envoy's `RetryPolicy` separates `route_action.timeout` (end-to-end, all retries)
from `RetryPolicy.per_try_timeout` (max duration for a single upstream attempt).
When an attempt exceeds `per_try_timeout`, Envoy aborts it and starts a retry,
subject to the global deadline. This lets operators express "5s total, but give
each backend only 1s before trying another node."

## Impact

- Retry config becomes ineffective when the first attempt is slow rather than
  immediately failing (`attempts: 3` may yield only 1 attempt).
- Operators cannot express a fast-fail per-attempt bound without globally
  reducing `backendRequest`, which also tightens healthy large-response
  transfers.
- gRPC streaming routes compound the issue via the `grpc-timeout` client
  deadline.

## Suggested direction (to be validated in brainstorming)

- Add a `perTryTimeout` field to `HTTPRouteRetry` (Edgion extension alongside
  `attempts` / `backoff` / `codes`), with the same Gateway API Duration schema
  pattern as the other timeout fields.
- Thread it into `ParsedRouteRetry` and the parse/convert layer.
- In `configure_peer_timeouts`, bound each attempt's read/write timeout by
  `per_try_timeout` (in addition to the existing `request_remaining` /
  `grpc_remaining` ceilings) so each attempt gets a fresh, bounded window while
  the outer deadline still caps the sum.

### Open design questions

- **Attempt scope**: Envoy's `per_try_timeout` applies to EVERY attempt
  (including the first), not only retries. The original finding's
  "`try_cnt > 1`" framing is questionable — decide whether per-try applies to
  the first attempt too.
- **`try_cnt` plumbing**: `configure_peer_timeouts` currently has no access to
  the retry attempt counter. Determine where the attempt index is available
  (Pingora `upstream_peer` per-attempt invocation / retry context) and how to
  surface it.
- **Interaction with `backendRequest`**: define precedence when both
  `backendRequest` and `perTryTimeout` are set.
- **CRD / docs / tests**: CRD regeneration, `docs/en` + `docs/zh` updates, and
  an integration test scenario covering "slow first attempt still leaves room
  for retries".

## Scope footprint

- Public CRD API: `HTTPRouteRetry` (user-visible).
- Layers: types (`http_route.rs`) + parse/convert + gateway runtime hot path
  (`pg_upstream_peer.rs`, `#[inline]`).
- Generated artifacts: CRD schema, en/zh docs, integration tests.
