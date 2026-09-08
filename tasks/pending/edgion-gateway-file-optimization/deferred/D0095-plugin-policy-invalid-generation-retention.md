# D0095 — Investigate Policy Enforcement During Invalid Generation Retention

## Meta

- Status: needs-investigation
- Severity: high (potential enforcement impact; not independently reproduced)
- Discovered in: B0132 policy-filter analysis
- Category: correctness
- Canonical task: [Investigate plugin policy and invalid-generation retention](../../../todo/investigate-plugin-policy-invalid-generation-retention.md)

## Evidence

`compile_plugins` computes `retain_previous_on_invalid_update` for invalid DSL/WAF construction before sealing the incoming policy gate. `PluginStore::apply_delta` and `replace_all` may then retain the entire prior activatable runtime. The analyst identified a possible combination of a new Block stamp and an unrelated retainable invalid entry that could leave an older unblocked generation serving. This is a code-path concern, not a reproduced defect or a conclusion about intended retention precedence.

## Why deferred

The policy adapter itself executes the supplied decision correctly. Resolving precedence belongs to generation compilation/publication and must consider the existing last-known-good contract across update and full replacement. Independent reproduction and requirements analysis precede any change.

## Next step

Follow the canonical task. Preserve existing integration tests; prefer deterministic publication tests and record any required new integration coverage separately.
