---
id: ARCH-014
status: candidate
kind: architecture-debt
priority: P3
primary_partition: PART-263
independent_verification: confirmed
---

# Remove the complete GatewayClass mirror maintained only for diagnostics

## Current consumers and avoidable ownership

`config/gateway_class/conf_handler_impl.rs` retains a process-global
RwLock<HashMap<String, GatewayClass>>. Full publication clones all object payloads
through arc_map_to_owned; delta publication repeats owned clones and add/update/
remove bookkeeping. Its only production read is count_gateway_classes, called
by the integration-only store-stats endpoint. Named and list getters are test-only
included support functions; their comments explicitly say production reads use
ConfigClient instead.

The existing ClientCache already owns each GatewayClass Arc and exposes a typed
DynClientCache count. Startup resolves actual classes through ConfigClient.
PluginPublisher separately retains those Arcs as required inputs for serialized
scope composition. The owned diagnostic mirror serves neither consumer.
Maintaining its full payload, lock and mutation protocol merely to count resources
adds an unnecessary third representation. Its tests chiefly verify that otherwise
unused mirror. This is cold-path retention and maintenance debt; no measured
throughput, hot-path allocation or current stale-admission defect is claimed.

## Bounded recommendation and alternatives

Remove the owned mirror and derive the existing gateway_class_store diagnostic
count from ConfigClient's GatewayClass DynClientCache, using its existing count
operation in store_stats. Keep the published response field if consumers need
it; the already reported client_caches.by_kind also exposes the same source count.
Deleting that response field is optional wider API work and is unnecessary here.

The handler still forwards exact incoming Arcs and delta removals to the canonical
PluginPublisher and triggers adaptive dependent reconciliation. It must not
query ClientCache from inside its callback: the caller holds the cache write
lock during publication. Only the separately invoked diagnostic handler reads
the cache. Do not replace the mirror with another atomic counter or registry.

Keeping an Arc-valued mirror would avoid deep object copies but leave the unused
ownership and update protocol. A diagnostic-only count could also be added to
publisher outputs, but that expands an independent owner when the API already
has ConfigClient. No EGC binding or publisher source store should be removed:
those perform actual generation-sensitive work.

## Invariants and future validation

Preserve full replacement, incremental source deltas, removals and same-key
recreation at the real publisher, source Arcs, class policy admission, adaptive
recovery and retained in-flight scopes. Diagnostics remain integration-gated and
authenticated. No cross-kind or raw-cache/runtime atomicity guarantee is added.
Keep cold configuration logs free of resource bodies or secrets.

Replace mirror-only assertions with handler-to-publisher behavior and canonical
cache count wiring. Cover full/partial/delete/wipe and class parameter/policy
effects; inspect existing shared publication tests before adding redundant cases.
Use the required private unit-test layout without widening production visibility.

## Original context and verification limits

All five configuration primaries were fully read, including EGC resolved policy,
stable weak bindings, deterministic globals and GatewayClass callbacks. Original
EGC hot-reload/publication, scoped policy and GatewayClass architecture were read.
The latter's claim that the config/gateway_class cache validates classes is stale:
current startup reads ConfigClient and scoped policy reads publisher inputs.
No current decision requiring the extra owned diagnostic mirror was found.

Evidence is static source and caller inspection; no source edits, tests, builds
or performance measurements were performed. Independent verification is pending.
