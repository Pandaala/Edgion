# Fail Closed on Plugin IP Matcher Limit Violations

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | correctness |
| Priority | P2 |
| Issue | N/A |
| Depends on | EdgionPlugins immutable publication and invalid-update policy |

## AI guide summary

Make the Gateway reject and mark nonactivatable any synchronized EdgionPlugins generation that exceeds the resource-wide IP matcher limit.

## Requirements

- Trace the shared schema validation, Gateway compilation, immutable store publication, last-known-good policy, and canonical reject-gate behavior.
- Preserve the single shared IP matcher implementation and `MAX_REFERENCED_IP_RULES` authority; do not add another counter or matcher.
- Ensure an active over-limit resource cannot remain activatable merely because the diagnostic currently exists only in `preparse_errors`.
- Define the resource-level invalid identity and update policy explicitly and verify mixed failures are deterministic.
- Keep detailed operator diagnostics out of fixed fail-close response bodies and preserve canonical access-log ownership.
- Do not modify existing integration-test files.

## Acceptance criteria

- Controller and Gateway produce the same validation diagnostic for the over-limit resource.
- The Gateway marks the generation nonactivatable and installs the canonical leading fail-close gate before upstream traffic.
- Valid-to-invalid hot replacement follows the explicitly reviewed update policy.
- Disabled and policy-stamped entries retain the schema-defined aggregate-count semantics.
- Focused unit/publication tests cover the exact boundary and one-over-limit cases; any necessary integration gap is tracked separately.
