# B0242 — HTTP Common Utilities

## Scope

- Pipeline state: analysis-in-progress; queued, no mutation authorized.
- Active anchor: `edgion-gateway`.
- Baseline: `3d7001c0`.
- Target: `edgion-gateway/src/plugins/http/common/http_util.rs`.
- Inspect complete shared HTTP utility parsing and formatting, request-path allocation, all callers and tests.
- Root owns Cargo and all writes. Existing integrations, external todos and handoff/WAF paths remain protected. Revalidate after the active batch before promotion.
