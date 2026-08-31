# Design Sensitive Ctx Provenance and Unmasking

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | security-design |
| Priority | P1 |
| Issue | N/A |
| Depends on | B0071 |

## AI guide summary

Define one cross-cutting provenance model that prevents sensitive values renamed through ctx storage from becoming eligible for access-log unmasking solely because of their destination key.

## Requirements

- Trace sensitive inputs through canonical `KeyGet` sources, CtxSet writes, later ctx reads, AccessLogExtern declarations, and final access-log rendering.
- Preserve the fixed hard-blocklist for canonical claims containers and the requirement that external fields be explicitly declared.
- Decide where provenance belongs so all ctx producers and consumers share one mechanism; do not add plugin-local masking flags or parallel access-log logic.
- Cover at least `Authorization`, `Cookie`, resolved secrets, canonical claims containers, nonsensitive headers, and derived constant/template values.
- Specify behavior for overwrite, cross-rule ctx reads, mapping/transform output, hot-reloaded plugin generations, and name collisions.
- Keep fail-close output free of secret values and preserve current observability restraint.
- Do not modify existing integration tests during design. If implementation needs end-to-end coverage, open a standalone integration task before changing the frozen suite.

## Acceptance criteria

- The design names one owner for sensitive provenance and one enforcement point for access-log unmasking.
- Every current ctx producer and AccessLogExtern consumer is inventoried with explicit migration behavior.
- Security invariants, compatibility decision, focused test plan, and any integration-test requirement are approved before implementation.
