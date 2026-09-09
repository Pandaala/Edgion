# B0229 — Runtime Listener Gateway Store

## Scope

- Pipeline state: ready to commit; sole serial lane, all gates passed.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/runtime/store/listener_gateway_info.rs`.
- Baseline: `7a8177b0`; only coordinator records changed after the analyst's `3775e7e5` freshness check.
- Analyst: `b0229_listener_store_analysis`.
- Inspect complete immutable flat/index publication, lookup consumers, listener specificity and Gateway generation identity. Earlier supporting edits and tests do not establish file coverage.
- Revalidate carried `listener-gateway-info-dual-arcswap-merged-snapshot.md` examples of old `Vec<GatewayInfo>` against current Arc ownership. Do not re-open known independent D0003/D0123 publication issues as local fixes.
- Writes authorized only to the two technical paths listed below, through a separate implementer. Existing integrations, external todos and protected handoff/WAF paths remain untouched.

## Analysis Outcome

- Analyst recommends local non-complex change at `1456c442`, target blob `f77ee331de82b493bda3af60849404b42af19fa2`; no Cargo run. Rebuild clones all annotations into a HashMap then reconstructs a BTreeMap for DNS, although the canonical helper accepts the original BTreeMap. Borrow it and retain only required owned output values; deterministic cold rebuild copy/allocation reduction, no request-throughput claim.
- Exact proposed paths: target annotation projection/inline tests/optional flat-type rustdoc; `skills/04-review/concurrency/regression-guards/listener-gateway-info-dual-arcswap-merged-snapshot.md` for current `Arc<Vec<Arc<GatewayInfo>>>` and historically scoped API claims. Do not touch B0228 architecture changes.
- Preserve one built/published snapshot and shared per-entry Arc identity, source listener order, named lookup/specificity-before-AllowedRoutes, physical address+protocol keys, status handling and hostname bytes. Separate get/get_index calls can straddle rebuilds; do not promise joint-epoch consistency for independent loads.
- Preserve annotation absence, exact true parsing, explicit empty versus missing plugin namespace, metrics strings and canonical DNS valid-ref precedence/invalid-ref fallback/inline normalization. Add local projection matrix for these and unrelated annotations; retain existing publication/identity tests and all integrations.
- Fresh filters before/after: listener store, route matcher, Gateway identity; optional canonical common DNS resolver-selection tests. Existing D0003/D0123 remain separate. No new issue, cache/index, tracing, metrics or shared API.
- Original knowledge: concurrency snapshot/cross-store pages, routing prefilter, Gateway store/ConfHandler architecture, ListenerKey/DNS contracts and coding/unit-test rules. Historical cross-store timing/consequence prose is not fresh evidence.
- Freshness: original analyst independently revalidated at `3775e7e5`; target blob and relevant knowledge/call chains remain unchanged. Merged CORS filtering does not affect annotation projection or parent-reference sharing. Direct simple implementation remains valid within the two-path allow-list.

## Implementation and Review

- Implementer `b0229_implementation` borrowed the original annotation map and removed both temporary map conversions. Added four local matrix tests; retained the ten existing store tests. Corrected current snapshot Arc ownership and independent-load caveats in the owning knowledge page.
- Fresh independent reviewer `b0229_final_review`: LGTM, no Must Fix or Should Fix, revalidated against external `312d10e8`. Reviewed source blob `aa332868b62d1c95e8b4ae3862664c1c8daf4830` and knowledge blob `7595a054fd9d3123a1bb2b8e7f07542b057d030c`; all original invariants and applicable red rules preserved.
- Fresh tests: listener store 14/14, route matching 19/19, Gateway identity 3/3, canonical common DNS 12/12. Workspace format/check/Clippy, agent docs, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff and reconciled frozen paths passed. Pre-merge or intermediate compilation results are not substituted for this batch's final gates.
- Concurrent external commits and exact frozen-path provenance: [reconciliation](../external-sync-312d10e8.md). No integration changes by this batch.
- Outcome: changed. Commit: `6e4af44e`; never pushed by the campaign.
