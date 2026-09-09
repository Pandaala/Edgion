# Refresh L4 Connect-timeout Knowledge

## Meta

- Status: pending
- Type: docs
- Priority: P3
- Source: Gateway campaign B0199 / D0113

## Acceptance Criteria

- Revalidate `skills/04-review/security/regression-guards/l4-relay-connect-timeout.md`
  against current TCP/TLS callers, outlier error classes, resource fields and CRDs.
- Separate historical FailureClass::Gateway and CRD-gap evidence from current
  LocalOrigin behavior; preserve useful original incident history explicitly.
- Do not reclassify already-bounded connect calls as missing-timeout defects or
  make runtime/schema changes under this documentation task.
