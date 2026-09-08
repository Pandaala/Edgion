# B0163 — gRPC Route Conf Handler

## Scope

- Candidate analysis HEAD: `17cd833b261161565fa9c25a8a5a3f1151f52e86`; freshness-revalidated and promoted unchanged at `78cf23d962f3fbc3ff4718e9c5fd1c40a24743d4`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/routes/grpc/conf_handler_impl.rs`
- Baseline blob: `574fc76fa211306969dd12b90365776b0f9a2da5`
- Exact implementation allow-list: owned file only

## Handoff

- Analysis: `b0160_final_review` read the complete gRPC route module, conf-sync/cache lifecycle, route manager/publication, schema inputs, sibling handlers, tests and precedents. It freshness-revalidated the unchanged target and B0162's shared parentRef call path at `78cf23d9`. Decision: change; simple one-file optimization.
- Value: every match item in one GRPCRoute rule currently rebuilds and retains an identical immutable `RouteMetricHandles` matrix. Because Gateway attachments and backend declaration-order labels are rule-level and contain no match identifier, constructing one Arc per rule removes `M × G × B × 15` duplicated counter handles plus optional histograms and label/allocation work without changing metric identity.
- Implementation brief: construct backend metric keys and one metric-handle Arc after the rule backends/selector, then clone it into catch-all or explicit-match units. Keep sharing per rule, preserve parsing, resolved/invalid rule behavior, specificity, plugin/backend state, access-log identity and atomic publication. Add pointer-identity coverage for two matches.
- Integration coverage: no new gap; immutable identity sharing is covered by focused unit tests and existing integrations remain frozen.
- Deferred: none; sibling compatibility alias remains for its own file review.
- Implementation: built one immutable `Arc<RouteMetricHandles>` after each rule's backend compilation and selector construction, then cloned it into that rule's catch-all or explicit-match units. Different rules retain separate matrices and their own backend-slot dimensions.
- Tests: added focused coverage proving same-rule pointer identity, cross-rule non-sharing, preserved rule/match IDs and service/method data, and two-versus-one backend metric slots. The full conf-handler module passed 5/5.
- Review: fresh independent reviewer `b0163_final_review` re-read the implementation and original rules, verified metric labels, backend slot mapping, route generation lifetime, matching/specificity/order/publication and access-log behavior, and returned LGTM with no Must Fix, Should Fix or deferred finding.
- Verification: `cargo fmt --all`; focused tests 5/5; locked workspace all-target check and Clippy; locked Gateway binary build; agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff and 3,107-file frozen-path guards passed. Clippy retained only existing warnings.
- Commit: `d9939375`; never pushed by the campaign.
