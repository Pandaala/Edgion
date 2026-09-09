# D0112 — L4 Timeout Pre-resolution

- Status: pending
- Severity: low
- Discovered in: B0199 TCP proxy analysis
- Canonical task: [Pre-resolve L4 listener timeouts](../../../todo/pre-resolve-l4-listener-timeouts.md)

TCP and both TLS proxy modes resolve configured timeout strings per connection;
TLS connect resolution can repeat within retries. This is redundant parsing, not
a missing-timeout defect. A shared listener-runtime design needs a separate
plan-first scope rather than a TCP-only cache.
