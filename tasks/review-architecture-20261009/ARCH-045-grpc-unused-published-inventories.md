---
id: ARCH-045
status: candidate
kind: architecture-debt
priority: P3
partition: PART-365
analyst: /root/analyze_outbound
independent_verification: confirmed
---

# Retire the unused route inventories beside the published gRPC match engine

## Concrete ownership and retained cost

`GrpcRouteRules` in `edgion-gateway/src/routes/grpc/routes_mgr.rs:25-28` publishes both `resource_keys: HashSet<String>` and `route_rules_list: Vec<Arc<GrpcRouteRuleUnit>>` beside its `match_engine`. `build_grpc_route_rules_from_units` (410-425) collects the unranked list and cloned resource keys, then clones the whole list into `GrpcMatchEngine::rebuild`. A successful publication retains the original list and key set in the outer rules object.

`GrpcMatchEngine` already owns the immutable ranked route array (`match_engine.rs:19,149`) addressed by every hostname and service/method numeric index. Its rebuild consumes the incoming vector. The previous generation is consulted only through `previous.grpc_routes.match_engine`, for hostname-tree reuse. Full and affected listener rebuilds (`routes_mgr.rs:226,299`) obtain the current units from `route_units_cache`; they never patch either published mirror. `stats` (363-376) counts the manager caches and uses the raw Route cache size for its `resource_keys` statistic, without reading the published HashSet.

A workspace-wide Rust caller search found no production consumer of either published mirror outside their construction and failure-count calculation. The remaining callers are test inspection: `grpc/routes_mgr_test.rs:14-15` checks their emptiness; `grpc/conf_handler_impl_test.rs:456,473` inspects pointer reuse through the unranked list; `http/proxy_http/pg_request_filter_test/request_entry_tests.rs:107-111` populates both mirrors when constructing a fixture. They do not establish a production inventory requirement.

For each published listener generation, this retains one extra Arc slot per compiled unit and a HashSet with cloned resource-name Strings, and adds a full-vector Arc/refcount clone on every successful rebuild. Retained old snapshots retain those mirrors until release. The statically identifiable work and storage scale with unit count and resource-key bytes. No heap-byte measurement, latency improvement or current incorrect route selection is claimed. The published snapshot exposes two route inventories in different orders although only the engine's ranked array participates in selection.

HTTP is deliberately outside this finding. `http/conf_handler_impl.rs::patch_domain_route_rules` (750-786) reads `resource_keys`, `route_rules_list` and `regex_routes` to replace touched resources while retaining unaffected units and path engines. Those lists have a current incremental consumer and cannot be removed by analogy.

## Bounded improvement and alternatives

Keep `route_units_cache` as the cold compile/runtime/selector retention owner, and keep the match engine's ranked array as the published matcher owner. Move the collected vector directly into `GrpcMatchEngine::rebuild` and stop retaining the unused list and key set in `GrpcRouteRules`. Compute resource and route counts as cold scalar values before moving the vector, preserving the existing bounded diagnostic and empty-table publication on build failure.

Preserve the tests' behavioral protection while changing their observation point. Empty-table checks should use `has_routes` and engine absence. Unchanged-unit reuse can be checked against the cached unit and a real match result, or a `cfg(test)` observer over the real engine where necessary. The existing engine test `reused_hostname_tree_never_reuses_ranks_from_the_previous_array` already inspects the actual ranked array and proves cached Arc retention. Request-entry fixtures can pass their units directly into the engine. Do not retain a production copy or add a public inventory accessor merely to preserve test convenience.

Retaining the mirrors is justified only if implementation finds a concrete current production consumer omitted by this audit. Prospective debugging or compatibility does not supply that requirement under the pre-release rules. Rearranging the engine to index a second shared vector would preserve dual order and ownership while expanding the matching change unnecessarily. No generic manager framework, cache consolidation, factory expansion or wrapper flattening is required.

## Invariants and verification limits

Preserve the outer ArcSwap publication, coherent rebuilding of ranked arrays and dependent candidate indexes, final-key hostname-tree reuse, unchanged cached unit/plugin/backend-selector Arcs, captured old snapshots, strict current-status/controller ownership, invalid-rule withdrawal, weighted invalid/denied slots, retained-manager empty clearing, native-gRPC fallback ownership and numeric compiled backend generations. HTTP mirrors, TLS policy ownership, main response streaming, request-only body storage, access-log ownership and existing logging remain unchanged.

Future verification should retain full/partial/Gateway rebuild and deletion tests, old/new matching results, unchanged runtime identity, build-failure clearing, and native-gRPC request-entry behavior. This is a cold storage and ownership reduction, not permission to add request-path introspection or allocation. External downstream API consumers were not inventoried; the project requires no compatibility shim for this pre-release refactor.

The analyst personally read all seven gRPC primary implementations, all HTTP and shared-L4 primaries, actual fixture and reuse tests, and the original method-matching, shared hostname-index, immutable-fields, route architecture and L4 carve-out records. The immutable-fields record accepts plain immutable fields; it does not require preserving unused gRPC inventories. The gRPC cache carve-out remains valid. Existing ARCH-018, ARCH-024 and ARCH-037 concern different owners and causes.

Static read-only review only. No builds, tests, CLI experiments, network calls, source edits, Git mutations or performance measurements were performed. Exact primary and support hashes and read scopes are recorded in the PART-365 terminal receipt; independent verification is pending.
