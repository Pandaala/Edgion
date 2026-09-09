# B0240 — HTTP Common Authentication

## Scope

- Pipeline state: analysis-in-progress; queued, no mutation authorized.
- Active anchor: `edgion-gateway`.
- Baseline: `f470e758`.
- Target: `edgion-gateway/src/plugins/http/common/auth_common.rs`.
- Inspect the complete shared HTTP authentication helper, credential/header ownership, failure response semantics, all plugin consumers and tests.
- Root owns Cargo and all writes. Existing integrations, external todos and handoff/WAF paths remain protected. Revalidate after the active batch before promotion.
