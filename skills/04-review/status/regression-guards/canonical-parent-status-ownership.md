---
name: canonical-parent-status-ownership
description: Review Route parent-status identity, controller ownership, semantic diff, and Gateway publication filtering.
knowledge_type: regression-guards
---

# Canonical parent status belongs to its configured controller

## Conclusion and scope

HTTPRoute, GRPCRoute, TCPRoute, TLSRoute and UDPRoute, plus EdgionTls status, identify parents
through the shared `ParentReference::canonical_identity`. Group defaults to the Gateway API group,
kind to Gateway, namespace to the referring resource namespace (then default). Explicit empty group,
different kinds, sections and ports remain distinct. `ParentStatusIdentity` adds controller ownership.

Controller `parent_status_for_update` only changes an owned canonical entry; retention prunes stale
and duplicate owned entries, preserving foreign entries from the observed snapshot. Condition updates
reuse the existing entry to preserve transition times. Empty/absent parentRefs also retains foreign
entries. Parent-status diff receives the resource namespace and compares entries one-to-one,
including multiplicity, so changes cannot hide behind a reused first match.

## Transport and Gateway consumption

Five Route specs carry the internal `resolvedStatusController` field. It is serde-transported but
excluded from operator schema via `schemars(skip)`. Each Controller handler unconditionally writes
its configured name in `finalize_transport`, after status generation and before cache publication.
Incoming values cannot override that source. The field relies on existing conf_sync authentication;
it is not a new proof or user-facing configuration setting.

Gateway route compilation and plugin publication filter using the stamp and status from the same
resource version. Missing/empty identity or duplicate matching owned entries rejects the attachment.
A single owned entry controls acceptance; foreign statuses never supply its result. A valid stamp
with no matching owned entry retains the existing optimistic no-status policy, including foreign-only
status input. Do not describe this as requiring owned Accepted=True in every case.

EdgionTls does not gain this field or a Route acceptance filter: its certificate validation and
attachment-proof checks remain independent. GatewayClass is not a new Route compilation dependency.
Existing requeue edges, worker/status-IO ownership and Gateway publication barriers remain unchanged.

## Rationale and limits

The defect combined partial-coordinate Controller lookup/retention/diff with a Gateway first-match
lookup that ignored controllerName. Resource-local ownership avoids adding class-arrival or session
identity invalidation to already-compiled route units. The previous [delegation collision guard](../../routing/regression-guards/delegation-parentref-gateway-name-collision.md)
remains valid and is extended by controller ownership, not replaced.

Snapshot preservation is not concurrent-writer safety: Kubernetes Merge Patch replaces the parents
array. A foreign update after our input snapshot may still be overwritten. This independent limitation
requires separate storage-layer investigation. The previously referenced follow-up
`tasks/todo/route-status-concurrent-writer-preservation.md` is absent from the repository;
this missing task does not establish that the limitation was resolved.

## Verification and re-evaluation

Fresh targeted verification on 2026-09-20:

- Resources `parent_identity`: 3 passed, including transport/schema coverage for all five specs.
- Controller `parent_status_identity_test`: 8 passed, including all six writers and five actual
  init/workqueue publication paths; `status_diff`: 10 passed, including malformed input.
- The broader Controller `resource_processor` suite passed 783 tests, including existing
  requeue, dropped-parent, coalesced-recreate and publication-order regression coverage.
- Gateway `routes::`: 729 passed; `plugins::publication::`: 21 passed, including all five families'
  unchanged-listener full/incremental owner and status changes.
- Independent code and knowledge review: LGTM after correcting malformed-container comparison.
- Closure checks passed: formatting, workspace all-target check and Clippy, agent docs, SSA force,
  metrics inventory, Pingora source policy and Gateway API transform contract. Clippy retained two
  pre-existing test-only warnings outside this diff (DSL gRPC redundant async block and Kafka
  err/expect). CRD structural/condition/type coverage passed 19/8/6 tests.

Existing `edgion-tests` files were unchanged. These are crate-level tests, not a fresh live
Kubernetes, etcd, or end-to-end dynamic run. Re-evaluate when parent defaults, ownership transport,
source-version ingestion, status persistence or publication caching changes.

## Resolution references

- `route-parent-status-canonical-identity`: canonical identity and configured ownership corrected;
  concurrent status persistence remains separately tracked above.
