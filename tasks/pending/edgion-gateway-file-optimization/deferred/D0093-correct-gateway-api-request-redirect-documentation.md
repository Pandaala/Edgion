# D0093 — Correct Gateway API RequestRedirect Documentation

## Meta

- Status: retired externally in `78a25c3a`; historical finding retained
- Severity: medium
- Discovered in: B0128 Gateway API redirect/rewrite analysis
- Category: documentation
- Former canonical task: `tasks/todo/correct-gateway-api-request-redirect-documentation.md`, removed by user-synchronized `78a25c3a` alongside the guide updates; do not resurrect it.

## Historical Evidence

The paired English and Chinese RequestRedirect guides do not accurately describe the supported `301`, `302`, `303`, `307`, and `308` status codes and include examples that combine a redirect-only rule with `backendRefs`. That obscures the terminal local-response contract and presents an invalid route shape as normal usage.

## Why deferred

These are standalone public-documentation corrections, not required to implement or verify B0128's path-element and downstream-TLS runtime fix. Keeping them separate prevents the four-file runtime batch from expanding into an unrelated guide rewrite.

## Integration-test impact

Documentation checks and existing redirect tests are sufficient. No integration issue is needed, and existing frozen integration files must not be modified.
