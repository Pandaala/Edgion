# D0115 — TLSRoute Stage-1 Fallback Removal

- Status: resolved (2026-09-10)
- Severity: medium
- Discovered in: B0205 TLS shared-module analysis
- Maintained outcome: [TLSRoute stage isolation](../../../../skills/04-review/tls/regression-guards/tls-route-stage-isolation.md)

The fallback was removed outside the frozen-test campaign under the user's
coordinated-fix authorization. TLSRoute now uses only Stage-2 plugins; nonempty
Stage-1-only attachments fail closed. Fixtures, documentation, and dynamic
coverage were migrated together, and the default integration sweep passed.
