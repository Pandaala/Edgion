# Adjudicate CSRF Expires Duration Representation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | design |
| Priority | P2 |
| Issue | N/A |
| Depends on | B0070 |

## AI guide summary

Decide whether the CSRF token lifetime supplies enough new evidence to reopen the closed seven-field GEP-2257 duration scope before changing its current positive integer-second contract.

## Requirements

- Read `skills/04-review/architecture/gep2257-duration-field-scope.md` and record which documented re-evaluation trigger, if any, the CSRF lifetime evidence satisfies.
- Separate the B0070 correctness fix for nonpositive/overflowing expiry from operator ergonomics; do not present a representation migration as necessary to retain B0070's security property.
- Inventory the resource schema, Gateway runtime, Controller/Gateway validation, EN/ZH docs, and integration fixtures affected by any representation decision.
- If migration is rejected, record the retained positive integer-second rationale and close without code changes.
- If migration is explicitly approved, use the shared GEP-2257 duration parser and update all layers atomically after the frozen campaign. Reject zero, sub-second values that cannot satisfy the token lifetime, and conversion overflow.
- Do not accept both integers and strings, add a deprecated alias, or preserve a compatibility shim.

## Acceptance criteria

- The closed duration-scope precedent is explicitly adjudicated with CSRF-specific evidence.
- The chosen representation and exact validation/runtime behavior are documented before implementation.
- Any approved implementation updates schema, validation, runtime, docs, and affected fixtures together and passes workspace checks and repository guards.
