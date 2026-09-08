# B0163 — gRPC Route Conf Handler

## Scope

- Candidate analysis HEAD: `17cd833b261161565fa9c25a8a5a3f1151f52e86`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/routes/grpc/conf_handler_impl.rs`
- Baseline blob: `574fc76fa211306969dd12b90365776b0f9a2da5`
- Exact implementation allow-list: owned file only

## Handoff

- Analysis: `b0160_final_review` read the complete gRPC route module, conf-sync/cache lifecycle, route manager/publication, schema inputs, sibling handlers, tests and precedents. The target matched its frozen baseline at external HEAD `d8d9d1d0`. Decision: change; simple one-file optimization.
- Value: every match item in one GRPCRoute rule currently rebuilds and retains an identical immutable `RouteMetricHandles` matrix. Because Gateway attachments and backend declaration-order labels are rule-level and contain no match identifier, constructing one Arc per rule removes `M × G × B × 15` duplicated counter handles plus optional histograms and label/allocation work without changing metric identity.
- Implementation brief: construct backend metric keys and one metric-handle Arc after the rule backends/selector, then clone it into catch-all or explicit-match units. Keep sharing per rule, preserve parsing, resolved/invalid rule behavior, specificity, plugin/backend state, access-log identity and atomic publication. Add pointer-identity coverage for two matches.
- Integration coverage: no new gap; immutable identity sharing is covered by focused unit tests and existing integrations remain frozen.
- Deferred: none; sibling compatibility alias remains for its own file review.
- Commit: pending; never push.
