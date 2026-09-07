# D0041 — Adjudicate CSRF Duration-string Migration

## Meta
- Status: accepted-tradeoff
- Severity: medium
- Discovered in: B0070, CSRF audit
- Category: schema-architecture
- Depends on: B0070
- Closed: 2026-09-05
- Canonical record: [`skills/04-review/csrf/intentional-design/csrf-expires-integer-seconds-retained.md`](../../../../skills/04-review/csrf/intentional-design/csrf-expires-integer-seconds-retained.md)

## Outcome
Migration rejected. `CsrfConfig.expires` stays a positive integer-second lifetime.
B0070's nonpositive/overflow fix is independent of representation and does not
reopen the closed GEP-2257 conversion set. No schema, runtime, docs, or fixture
change.

## Integration-test impact
None. D0042 Secret-rotation coverage remains independent.
