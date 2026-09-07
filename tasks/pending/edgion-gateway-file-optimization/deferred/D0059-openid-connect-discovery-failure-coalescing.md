# D0059 — Coalesce and Back Off Failed OpenID Connect Discovery Fetches

## Meta
- Status: resolved
- Severity: medium
- Discovered in: B0092, OpenID Connect final review
- Category: reliability-concurrency
- Closed: 2026-09-05
- Canonical record: [`skills/04-review/oidc/regression-guards/discovery-failure-backoff.md`](../../../../skills/04-review/oidc/regression-guards/discovery-failure-backoff.md)

## Evidence
The discovery refresh mutex serializes cold callers, but a failed discovery fetch published no shared outcome. After the first failure released the mutex, each waiting caller observed an empty cache and performed another fetch, so a burst could produce one provider timeout per request.

## Resolution
Failed discovery now publishes a 5-second typed backoff. Concurrent and immediately following callers reuse that error without contacting the provider; a call after the deadline retries and a successful recovery publishes one new `Arc` snapshot. Constructor prewarm is unchanged. A cancellation-safe Shared Future, an operator rediscovery knob, and permanent negative cache were rejected.

## Integration-test impact
No new integration issue. Provider-local Tokio fixtures cover backoff reuse, concurrent failure, retry, and recovery. Existing frozen integration tests remain unchanged.
