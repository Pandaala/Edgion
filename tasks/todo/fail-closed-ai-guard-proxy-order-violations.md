# Fail Closed on Invalid AiGuard/AiProxy Ordering

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | security-correctness |
| Priority | P1 |
| Issue | N/A |
| Depends on | Shared AiGuard ordering validation and EdgionPlugins publication |

## AI guide summary

Prevent an EdgionPlugins generation with AiGuard ordered after AiProxy from becoming active in the Gateway.

## Requirements

- Reuse `collect_ai_guard_ordering_errors` as the single ordering-policy authority in both Controller and Gateway paths.
- Trace request-stage construction, immutable store activation, hot replacement, and the canonical leading reject gate.
- Ensure the ordering violation marks the generation nonactivatable and cannot forward an unguarded prompt upstream.
- Define the resource-level invalid identity and update policy explicitly, including mixed failures and multiple offending entries.
- Preserve inactive and policy-stamped entry semantics, diagnostic redaction, observability restraint, and access-log ownership.
- Do not modify existing integration-test files.

## Acceptance criteria

- Controller and Gateway retain exact diagnostic parity for every ordering violation.
- The Gateway installs the canonical fail-close gate before any request plugin or upstream dispatch and marks the generation nonactivatable.
- Valid-to-invalid and invalid-to-valid hot replacements behave according to the reviewed update policy.
- Focused publication tests cover single and multiple ordering violations plus inert entries; any necessary integration gap is tracked separately.
