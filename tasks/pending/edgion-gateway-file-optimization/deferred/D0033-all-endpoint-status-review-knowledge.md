# D0033 — Refresh AllEndpointStatus Outbound-header Review Knowledge

## Meta
- Status: needs-implementation
- Severity: low
- Discovered in: B0063, AllEndpointStatus audit
- Category: knowledge-maintenance
- Canonical task: [`tasks/todo/refresh-all-endpoint-status-header-review-records.md`](../../../todo/refresh-all-endpoint-status-header-review-records.md)

## Evidence
The current AllEndpointStatus implementation filters `Content-Length`, `Transfer-Encoding`, and `Expect` before constructing its plugin-owned outbound requests. Three review records still claim that it forwards one or more of these headers and reference a missing follow-up task. User-facing summary wording also describes `failure_count` as non-2xx rather than non-2xx/3xx.

## Why deferred
These are documentation and closed-review corrections, not runtime defects. Editing review knowledge inside B0063 would mix maintenance scope with a security/correctness implementation batch.

## Integration-test impact
None. The maintenance task should verify the current header filter and summary contract against source and focused tests; no integration file needs modification.
