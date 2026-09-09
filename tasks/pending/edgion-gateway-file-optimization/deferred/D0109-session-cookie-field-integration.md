# D0109 — Session Cookie Field Integration

- Status: resolved (2026-09-09)
- Severity: medium
- Discovered in: B0195 session persistence analysis
- Maintained knowledge: [Session persistence token extraction](../../../../skills/04-review/routing/regression-guards/session-persistence-token-extraction.md)

The separately authorized test-only change covers an affinity token in the
second Cookie field and malformed expiry refresh over HTTP/1.1 and gRPC HTTP/2.
Both existing suites passed 5/5 cases, including backend identity and no-refresh
assertions. The maintained knowledge page owns the verification and its limits.
