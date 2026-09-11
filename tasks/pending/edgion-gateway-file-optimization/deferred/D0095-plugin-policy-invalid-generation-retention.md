# D0095 — Investigate Policy Enforcement During Invalid Generation Retention

## Meta

- Status: needs-implementation (narrow HTTP LKG defect mitigated in B0301; broader migration pending)
- Severity: high
- Discovered in: B0132 policy-filter analysis
- Category: correctness
- Canonical task: [Investigate plugin policy and invalid-generation retention](../../../todo/investigate-plugin-policy-invalid-generation-retention.md)

## Evidence

B0301 confirmed the HTTP publication defect and made whole-runtime LKG ineligible whenever either generation contains an enabled Controller policy stamp. Deterministic delta/full tests now cover Block, Bypass, removal and policy changes with invalid DSL/WAF generations.

## Why deferred

Concurrent user work expanded the canonical task into the broader migration from Controller startup policy to GatewayClass-scoped `EdgionGatewayConfig.spec.pluginPolicy`. B0301 does not implement that schema, dynamic publication, Controller stamp removal, stream/TLS-route coverage, documentation, or migration verification.

## Next step

Keep the canonical task and this backlink open until the broader migration is implemented, independently reviewed and verified. Do not duplicate the task or treat the B0301 HTTP mitigation as migration completion.
