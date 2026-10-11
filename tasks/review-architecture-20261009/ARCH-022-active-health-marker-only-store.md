---
id: ARCH-022
status: candidate
kind: architecture-debt
priority: P2
primary_partition: PART-254
independent_verification: confirmed
---

# Keep health status markers without a second threshold engine

## Production role and test ownership

health/check/status_store.rs retains hash-keyed BackendHealthState, counters,
timestamps, unhealthy snapshots and mutation APIs. Production HealthCheckManager
only registers register_service(key, Vec::new()) and unregisters markers during
serialized reconciliation; admin reads tracked_service_count. Workspace Rust
searches find no production record_check or is_healthy consumer. Probe completion
updates ServiceDiscoveryCoordinator::record_health_completion; routing reads
identity-bound EndpointRuntimeState in a captured runtime generation.

Each marker nevertheless owns two empty ArcSwap sets and an endpoint-state mutex.
Eight compatibility tests exercise this second threshold engine. Changing active
health counter reset or transitions consequently requires assessing two algorithms;
legacy-only regression changes can leave the real coordinator policy untested.
This is duplicate policy/test ownership, not a reported threshold/cancellation bug.

## Bounded improvement and alternatives

Retain a marker-only registry and its existing admin count/key behavior. Keep
membership mutation in serialized reconciliation. Remove the unused hash endpoint
engine and routing-looking APIs after a fresh caller audit. Migrate useful
consecutive failure/success, counter-reset and cross-Service isolation assertions
to actual coordinator/runtime fixtures using canonical endpoint identities.
Preserve unit granularity and the existing manager marker-lifecycle regression.

Do not delete useful coverage, move marker writes into probes, infer markers from
health status or restore global lookup in selection. A delegating test fixture can
simplify setup; a second production algorithm solely for tests is the ownership
cost. API-only retired assertions require explicit justification before removal.

## Invariants, risks and future checks

Preserve coordinator threshold policy, membership/endpoint/policy fencing,
completion/retirement locks, stale-result rejection and cancellation. Aborted
synchronous work must not recreate removed markers. Existing admin residue checks
remain meaningful. ARCH018 addresses a separate passive-outlier twin.

Future cases must exercise threshold transitions/reset/isolation on the real
state, late completion after replacement/removal, same socket in different
sections, reconciliation without a Tokio runtime, manager cancellation and zero
admin residue after removal.

## Prior decision and review limits

The analyst fully read all seven health primaries, actual probe/completion/routing
callers and relevant tests. Original active-health-task-status-ownership explicitly
records hash-engine retention for isolated compatibility tests. This proposal
accepts its marker race correction and revisits that retained ownership cost;
the original record supplies neither a production consumer nor an invariant that
needs a second threshold engine. Independent review must assess that scoped
decision rather than silently override it. service-runtime-snapshot-selection
forbids restoring request-path lookup; test pruning rules require useful coverage
to survive. No executions or performance measurements; verification pending.
