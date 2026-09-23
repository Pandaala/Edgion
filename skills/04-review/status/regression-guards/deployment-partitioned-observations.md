---
name: deployment-partitioned-observations
description: Shared resource status isolates deployment-local outcomes while preserving native local state and concurrent observations.
knowledge_type: regression-guards
---

# Shared status isolates deployment observations

## Trigger and root cause

Deployments with overlapping resource watches can see different Secrets,
ReferenceGrants or backends. A fresh resourceVersion precondition prevents stale
replacement but does not stop alternating, successful writes of opposite locally
computed conditions. Sharing controllerName across different GatewayClasses makes
controllerName-only native attachment ownership insufficient too.

`CAND-P2-B` concerns EdgionPlugins, EdgionStreamPlugins, EdgionBackend and LinkSys.
`CAND-P2-A` concerns class-dependent native attachment outcomes; its companion
rule is [canonical parent ownership](canonical-parent-status-ownership.md).
`CAND-P4-01` is corrected at ACME handler admission and service eligibility through
the shared Gateway/class ownership proof. Foreign deployments neither publish
the resource nor schedule transient eligibility retries. Scheduler/in-flight
fencing remains separately tracked and is not claimed fixed by this status envelope.

## Corrected contract

- Kubernetes stores complete `ControllerStatus<T>` observations in controllers[],
  keyed by controllerName and gatewayClassName. gatewayClassUid proves incarnation;
  it is not a new key. LinkSys lifecycle fields stay inside the same nested payload.
- Handlers compute with native working status. ServerCache/conf_sync retain the
  complete logical `status` and a separate typed native `spec.currentStatus` for
  current-deployment decisions. Gateway status consumers use currentStatus only.
  The processor returns a separate persistence payload; Kubernetes must not
  extract it from local cache. Typed status replacement preserves compiled fields
  and ingest sidecars. This split implements `current-status-projection`.
- A matching committed raw GatewayClass/controller/nonempty UID identifies a flat
  observer without granting serving authority. Native serving additionally needs
  an owned raw Gateway and matching published UID/class.
- Merge preserves foreign entries and their order. Cleanup requires authoritative
  class identity/deletion evidence; absence requires a complete unfiltered LIST.
  Source teardown and filtered/unwatched views do not prove deletion. **This page
  governs the private envelope**, including this cleanup; the Gateway API native
  lists are governed separately by
  [foreign status entry preservation](../mandatory-design/foreign-status-entry-preservation.md),
  whose upstream contract forbids reclaiming a foreign entry and keys on
  controllerName alone. The envelope's divergence is deliberate and outside that
  contract — these are Edgion's own CRDs — so it is not a finding. Whether to
  align them is an open proposal, not a decision; that page's "Relationship to
  the private envelope" section records the retired premise behind the class key.
- The cap is 32 after pruning. Existing owners can update at 32; new owners cannot
  evict others. Invalid observations or capacity failure suppress persistence,
  not local configuration processing. Malformed input is retained opaquely until
  the processor can reject its status without rejecting the whole resource.
  When persistence is suppressed, retain the observed complete status and the
  independently computed currentStatus. Incoming currentStatus is overwritten;
  it is transport-only, excluded from operator schemas and API status patches.
- Recognized native-only input is not inherited by any deployment. Canonicalization
  sends fixed-whitelist native-field null tombstones only on the wire. All retired
  fields must be cleared, including empty/null fields omitted by typed serialization.
  Unknown or mixed input does not authorize arbitrary field deletion.
- Semantic comparison matches observation keys one-to-one, compares UID and native
  payload meaning, and ignores order and condition transition times. Lifecycle
  values remain semantic. CAS, one worker and inline status IO are unchanged.

## Recovery and verification limits

Committed raw GatewayClass/Gateway/HTTPRoute/GRPCRoute identity deltas enqueue raw
dependents, including objects never admitted to the processed cache. Status/RV
echoes do not fan out. Gateway publication fingerprints use the exact validated
Gateway/class incarnation, ensuring recovery when a dependent ran between raw and
processed publication even if listeners did not change.

Historical 2026-09-21 checks for observation isolation, before currentStatus:

- Controller status-related tests: 161 passed; processor suite: 819 passed.
- Additional typed-empty/native-field retirement regression: passed.
- Resource envelope/CRD guards: 5 passed; shared wrapper tests: 2 passed.
- Ownership tests: 11 passed; GatewayRouteIndex: 16 passed; ACME eligibility: 3 passed.

These are in-process transport, queue and schema tests, not live Kubernetes/ACME
or full end-to-end integration results. Final workspace all-target check, Clippy,
formatting, documentation, SSA, metrics, Pingora source and transform-contract
guards passed on 2026-09-21; CRD coverage checks passed (19 + 8 tests). Clippy
retained two unrelated existing warnings. A fresh independent read-only code and
knowledge review returned LGTM. The completed observation-isolation and ACME
admission tasks were removed. ACME scheduler limits remain documented below.
Independent deployments claiming the same class still need the same election
domain and configuration view. The existing native ancestor capacity limit is not
permission to truncate foreign entries. ACME timers, restart recovery and active
attempt handoff fencing remain outside the verified correction.

The subsequent [GatewayClass reporting correction](gatewayclass-reporting-and-publication.md)
requires distinct controllerName values for independent deployments, including
those selecting different classes. Observation partitioning remains unchanged;
sharing a controllerName across independent election/configuration domains is no
longer a supported deployment contract.

## Re-evaluation

The subsequent `current-status-projection` change was independently reviewed and
verified on 2026-09-21: final Controller processor tests 821, current-status tests
18, ACME tests 33, transport/schema tests 3 (17 resource schemas and four custom
deserializers), structural/condition/type CRD checks 19/8/6. Workspace checks and
all repository guards passed; live Kubernetes/etcd and GatewayDynamic were not
run. See [canonical parent ownership](canonical-parent-status-ownership.md) for
Gateway test results and the separate unresolved reporting requirement.

Revisit on new status resources, changed payload schemas, controller identity or
watch-scope changes, new serving consumers, dependency fanout changes, or a new
status writer bypassing the canonical pipeline. Canonical authority:
[status management](../../../01-architecture/01-controller/10-status-management.md).
