# B0231 — TLS Outbound Client

## Scope

- Pipeline state: analysis-ready; queued, no mutation authorized.
- Active anchor: `edgion-gateway`.
- Baseline: `7a8177b0`.
- Target: `edgion-gateway/src/tls/validation/outbound_client.rs`, introduced externally by `7ceb0ea1`.
- Inspect complete target, actual callers, canonical outbound TLS/egress ownership and relevant validation knowledge. Unsupported Gateway rustls-only gaps are out of scope.
- Root owns Cargo and all writes. Integrations, external todo work and handoff/WAF paths remain protected. Do not promote before B0230 completes its commit gate.

## Analysis Outcome

- Analyst `b0231_outbound_client_analysis` recommends defer at external HEAD `879df86f`; target blob unchanged, `858eda1d668d70d15d0779a7da15fa6d74219576`. No useful isolated optimization found.
- Follow-up candidate: each custom CA bundle must yield a certificate, not merely the aggregate across bundles. A valid bundle masks an empty declared bundle in this target and common `build_rustls_client_config`/`apply_custom_ca_pem`; canonical resolution permits empty `ca.crt`. This silently accepts partially unusable configuration, not unauthorized trust or a chain-verification bypass.
- No existing standalone duplicate found. After promotion and independent validation, create `tasks/todo/reject-empty-individual-outbound-ca-bundles.md`; cross-cutting repair requires plan-first scope across this target, common TLS adapters, inline tests and owning OIDC construction knowledge. Do not patch a single adapter during this candidate.
- Original rules/knowledge: updated root AGENTS, design rules, review routing, outbound unification, Rust/unit tests, TLS/OIDC indices, OIDC fail-closed construction and dial-time egress, AllEndpointStatus identity. Verified explicit ring provider, system/custom root exclusivity, base verification before SAN constraints, mTLS fail-close, canonical client/dial policies and intentional verified handling of `verify=false`.
- Suggested fresh filters: Gateway outbound_client, outbound_san_verifier, `oidc_san_`, `oidc_construction_fails_closed`; common `link_sys_core::tls::tests`. No Cargo or writes by analyst. This backend-independent rustls client is not the unsupported Gateway backend.
