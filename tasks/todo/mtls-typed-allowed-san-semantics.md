# Define typed mTLS allowed SAN semantics

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Define and enforce type-aware matching for `EdgionTls.spec.clientAuth.allowedSans`. The runtime currently flattens DNS, IP, URI, and email certificate SANs into strings and applies DNS hostname matching to every entry. That makes identity semantics ambiguous and can incorrectly apply ASCII case folding or wildcard behavior to non-DNS identities.

## Requirements

- Freeze the operator-facing syntax for DNS, IP, URI, and email allowlist entries.
- Preserve RFC 6125 single-label wildcard and ASCII case-insensitive matching only for DNS names.
- Parse IP entries and compare normalized address values exactly.
- Define explicit, standards-backed comparison rules for URI and email identities; do not apply DNS wildcard semantics to them.
- Reject malformed typed entries during Controller validation while retaining conservative fail-close Gateway enforcement.
- Use one canonical typed matcher for validation and runtime consumption.

## Scope

### In scope

- `EdgionTls.spec.clientAuth.allowedSans` schema, validation, documentation, and runtime matching.
- Certificate SAN extraction representation if a typed representation is required.
- Controller/Gateway parity tests and operator-facing examples.

### Out of scope

- Common Name matching.
- BackendTLSPolicy SAN pinning.
- Gateway rustls backend support or parity.

## Acceptance criteria

- DNS exact and single-label wildcard cases are covered, including ASCII case folding.
- IP comparison is based on parsed address identity, including canonical IPv6 forms.
- URI and email tests lock in their documented exact matching semantics and reject DNS wildcards.
- A certificate SAN of one type cannot match an allowlist entry of another type.
- Malformed typed allowlist entries surface `Accepted=False` and cannot reach the Gateway as an active permissive rule.
- Targeted resource and Gateway TLS tests, workspace check, Clippy, and repository guards pass.

## Related guidance

- `skills/00-design-rules.md`
- `skills/01-architecture/02-gateway/04-tls/SKILL.md`
- `skills/02-features/03-resources/09-edgion-tls.md`
- `skills/05-testing/SKILL.md`

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-31 | Defer the change from the file audit. | A local string-comparison patch would guess operator-facing identity semantics and leave Controller validation, documentation, and Gateway runtime inconsistent. |
