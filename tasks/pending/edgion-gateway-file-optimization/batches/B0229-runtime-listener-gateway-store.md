# B0229 — Runtime Listener Gateway Store

## Scope

- Pipeline state: analysis-ready; queued, not active mutation.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/runtime/store/listener_gateway_info.rs`.
- Baseline: `3775e7e5` (freshness revalidated after the remote CORS merge).
- Analyst: `b0229_listener_store_analysis`.
- Inspect complete immutable flat/index publication, lookup consumers, listener specificity and Gateway generation identity. Earlier supporting edits and tests do not establish file coverage.
- Revalidate carried `listener-gateway-info-dual-arcswap-merged-snapshot.md` examples of old `Vec<GatewayInfo>` against current Arc ownership. Do not re-open known independent D0003/D0123 publication issues as local fixes.
- No writes authorized. Existing integrations, external todos and protected handoff/WAF paths remain untouched.

## Analysis Outcome

- Analyst recommends local non-complex change at `1456c442`, target blob `f77ee331de82b493bda3af60849404b42af19fa2`; no Cargo run. Rebuild clones all annotations into a HashMap then reconstructs a BTreeMap for DNS, although the canonical helper accepts the original BTreeMap. Borrow it and retain only required owned output values; deterministic cold rebuild copy/allocation reduction, no request-throughput claim.
- Exact proposed paths: target annotation projection/inline tests/optional flat-type rustdoc; `skills/04-review/concurrency/regression-guards/listener-gateway-info-dual-arcswap-merged-snapshot.md` for current `Arc<Vec<Arc<GatewayInfo>>>` and historically scoped API claims. Do not touch B0228 architecture changes.
- Preserve one built/published snapshot and shared per-entry Arc identity, source listener order, named lookup/specificity-before-AllowedRoutes, physical address+protocol keys, status handling and hostname bytes. Separate get/get_index calls can straddle rebuilds; do not promise joint-epoch consistency for independent loads.
- Preserve annotation absence, exact true parsing, explicit empty versus missing plugin namespace, metrics strings and canonical DNS valid-ref precedence/invalid-ref fallback/inline normalization. Add local projection matrix for these and unrelated annotations; retain existing publication/identity tests and all integrations.
- Fresh filters before/after: listener store, route matcher, Gateway identity; optional canonical common DNS resolver-selection tests. Existing D0003/D0123 remain separate. No new issue, cache/index, tracing, metrics or shared API.
- Original knowledge: concurrency snapshot/cross-store pages, routing prefilter, Gateway store/ConfHandler architecture, ListenerKey/DNS contracts and coding/unit-test rules. Historical cross-store timing/consequence prose is not fresh evidence.
- Freshness: original analyst independently revalidated at `3775e7e5`; target blob and relevant knowledge/call chains remain unchanged. Merged CORS filtering does not affect annotation projection or parent-reference sharing. Direct simple implementation remains valid within the two-path allow-list.
