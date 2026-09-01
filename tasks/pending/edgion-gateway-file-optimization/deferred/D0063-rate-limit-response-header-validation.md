# D0063 — Validate Rate-Limit Response Header Names Canonically

## Meta
- Status: needs-implementation
- Severity: medium
- Discovered in: B0096, RateLimit analysis
- Category: configuration-validation
- Canonical task: [`tasks/todo/validate-rate-limit-response-header-names.md`](../../../todo/validate-rate-limit-response-header-names.md)

## Evidence
The shared `LimitHeaderNames` accepts any non-empty string. RateLimit and RateLimitRedis later pass those values to response-header writers and ignore insertion errors. Invalid HTTP names are therefore silently omitted, case-insensitive duplicates are ambiguous, and framing or hop-by-hop names such as `content-length` can conflict with the generated local response body or the proxied request lifecycle.

## Why deferred
The type is shared by two plugins and admission must remain identical in resources, Controller preparse, and Gateway preparse. A plugin-local runtime filter would preserve invalid accepted configuration and let the two consumers drift. The canonical validation and documentation update require a separate cross-cutting, plan-first batch.

## Integration-test impact
No new integration issue is needed. Schema validation, Controller/Gateway parity tests, and colocated response-writer tests can cover the contract while existing integration tests remain frozen.
