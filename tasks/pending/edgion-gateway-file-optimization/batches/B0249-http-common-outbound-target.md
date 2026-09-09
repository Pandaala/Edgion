# B0249 — HTTP Common Outbound Target

## Scope

- Pipeline state: analysis-ready; complex change candidate refreshed at `98b88473`, awaiting serial promotion.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `a9236e7579d09198c4c5315929103cfcfef0d9da`.
- Target: `edgion-gateway/src/plugins/http/common/outbound/target.rs`.
- Target blob: `9589610f0f9fe1b31ed79d28dfcfa8ddc43ced3e`.
- Read scope may expand across the workspace to resolve target selection, Service/URL identity, configuration admission, DNS and final-dial ownership.
- Frozen write scope is not yet authorized; analysis is read-only.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: change; complex/plan-first.
- `WebhookServiceRef.port` is a Kubernetes Service port, but the current compatibility facade compares it directly with raw EndpointSlice serving ports. This rejects valid `443 -> 8443` mappings and can select an unrelated sibling section whose serving port happens to equal `443`.
- Reuse the existing immutable `ServiceDiscoveryCoordinator` section proof in `EpSliceStore::ready_endpoints_for_port`, returning only canonical ready slots from `ServiceSectionKey::Number`. Missing, invalid, ambiguous, empty or non-inet sections fail closed without raw-slice/DNS fallback.
- Exact proposed code allow-list: target `edgion-gateway/src/plugins/http/common/outbound/target.rs` plus necessary active-anchor support `edgion-gateway/src/backends/discovery/endpoint_slice/ep_slice_store.rs`, including inline unit tests. No cross-crate or integration edit.
- Preserve call-time generation refresh, local round robin, B0246 per-candidate pinned validation and final literal validation, RFC1918 Service allowance, URL/DNS behavior, redirects/proxy policy, retry and health/circuit ownership.
- Decisive knowledge: outbound unification, canonical Service discovery ownership, named/numbered Service-port resolution and final-dial SSRF regression guards. D0127 remains independent; no new deferred issue.
- Refresh at `98b88473`: target blob remains `9589610f0f9fe1b31ed79d28dfcfa8ddc43ced3e` and proposed support blob is `c7f79366d0454f1fd4cc9885df0401935ccc0b43`. B0247 changed only egress TTL/cache arithmetic, so the defect, exact two-file scope, invariants and plan-first classification remain unchanged.
