# D0009 — Route Retry and Pingora Loop Ceiling

## Meta

- Status: resolved
- Severity: medium
- Discovered in: B0056, `edgion-gateway/src/routes/http/proxy_http/pg_fail_to_connect.rs`
- Category: retry configuration contract

## Evidence

The locked Pingora proxy loop defaults to 16 iterations, while ordinary HTTP and
GRPC route retries and `EdgionGatewayConfig.spec.maxRetries` accept values above
that ceiling. Existing config tests intentionally retain values such as 99, so
Edgion can publish a retry count that the proxy loop cannot execute. AI explicit
route retries are separately capped and the global AI fallback is clamped, so
the mismatch primarily affects the ordinary route/config contract.

## Why deferred

Resolving the mismatch requires one explicit contract across schema validation,
Gateway config resolution, route policy, listener/server construction, user
documentation, and the locked Pingora loop. It is not safe to clamp one callback
or silently change accepted configuration inside B0056.

## Resolution

- `skills/04-review/routing/regression-guards/retry-budget-owns-pingora-loop-capacity.md`
