# D0041 — Adjudicate CSRF Duration-string Migration

## Meta
- Status: needs-discussion
- Severity: medium
- Discovered in: B0070, CSRF audit
- Category: schema-architecture
- Depends on: B0070
- Canonical task: [`tasks/todo/migrate-csrf-expires-to-duration-string.md`](../../../todo/migrate-csrf-expires-to-duration-string.md)

## Evidence
`CsrfConfig.expires` is an operator-facing integer-second lifetime. B0070 closes the demonstrated nonpositive-lifetime replay bypass and adds checked runtime enforcement, but the retained integer shape remains outside the default form for a newly designed duration field.

## Why deferred
The closed GEP-2257 duration-scope precedent intentionally retained every integer duration outside a seven-field conversion set. The CSRF-specific defect is new evidence to discuss against that precedent, not automatic authorization to migrate. Any approved migration must be atomic across schema, Gateway, docs, and integration fixtures after this campaign's frozen-test boundary, with no integer/string compatibility form.

## Integration-test impact
No integration issue is opened solely for this discussion. If migration is approved, its implementation task must update affected fixtures and keep the separate D0042 rotation scenario contract intact.
