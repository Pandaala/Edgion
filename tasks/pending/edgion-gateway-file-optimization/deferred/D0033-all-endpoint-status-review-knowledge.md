# D0033 — Refresh AllEndpointStatus Outbound-header Review Knowledge

## Meta
- Status: resolved
- Severity: low
- Discovered in: B0063, AllEndpointStatus audit
- Category: knowledge-maintenance
- Closed: 2026-09-10
- Maintained review records: [outbound expectation filtering](../../../../skills/04-review/forward-auth/regression-guards/outbound-expect-continue-not-relayed.md), [hop-by-hop stripping inventory](../../../../skills/04-review/http1/regression-guards/hop-by-hop-dynamic-stripping.md)

## Evidence
The current AllEndpointStatus implementation filters `Content-Length`, `Transfer-Encoding`, and `Expect` before constructing its plugin-owned outbound requests. Its summary counts only 2xx/3xx responses as successes; other statuses and transport or time-limit failures increment `failure_count`.

## Resolution
Commit `ac3d8951` aligned the maintained ForwardAuth and HTTP/1 review records with the independent AllEndpointStatus request builder and corrected the English and Chinese summary definitions. Closure revalidation ran the focused header-filter and ordinal-summary unit tests, checked the maintained links and stale wording, and passed agent-document validation and `git diff --check`. The temporary canonical todo was removed after these checks; the maintained review records above are the durable references.

## Integration-test impact
None. Focused unit tests cover the header filter and summary contract; no integration file was modified.
