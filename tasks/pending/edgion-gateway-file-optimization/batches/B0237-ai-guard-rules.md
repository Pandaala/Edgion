# B0237 — AI Guard Rules

## Scope

- Pipeline state: analysis-in-progress; queued, no mutation authorized.
- Active anchor: `edgion-gateway`.
- Baseline: `899436e7`.
- Target: `edgion-gateway/src/plugins/http/ai_guard/rules.rs`.
- Inspect complete rule compilation/evaluation, configuration semantics, regex ownership, callers and tests. B0236 separately owns plugin/body flow.
- Root owns Cargo and all writes. Existing integrations, external todos and handoff/WAF paths remain protected. Revalidate after B0236 before promotion.
