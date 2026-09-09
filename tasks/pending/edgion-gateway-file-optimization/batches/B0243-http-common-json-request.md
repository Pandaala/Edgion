# B0243 — HTTP Common JSON Request

## Scope

- Pipeline state: analysis-in-progress; queued, no mutation authorized.
- Active anchor: `edgion-gateway`.
- Baseline: `761ea605`.
- Target: `edgion-gateway/src/plugins/http/common/json_request.rs`.
- Inspect complete shared JSON request construction, byte bounds, error semantics, all callers and tests.
- Root owns Cargo and all writes. Existing integrations, external todos and handoff/WAF paths remain protected. Revalidate after the active batch before promotion.
