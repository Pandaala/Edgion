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

ControllerName alone is insufficient when independent deployments manage different
GatewayClasses under the same name. Kubernetes `status_ownership` projection now
separates the local cache status from the persisted native array: only proven local
Gateway attachments contribute local state, while foreign controllers and other
classes remain in the persisted snapshot. Missing Gateway history is retained
without granting serving authority. The same projection handles both policy
ancestor families. Raw spec removal can consistently clear this controller's
removed parent entries without erasing another controller's entries.

HTTP/GRPC reverse-lock diagnostics keep their local meaning but use one persisted
writer selected from the first sorted canonical explicit parentRef before lookup.
Missing, invalid, cyclic or over-depth chains do not select another visible branch.

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
Worker/status-IO ownership remains unchanged. Kubernetes serving resolution now
requires an owned raw Gateway and a matching published nonempty UID/class, shared
by Route, TLS and ACME consumers. Committed raw ownership changes requeue raw
dependents, including never-published objects. Post-publication Gateway/class UID
fingerprints restore dependents that ran before the new projection was available;
unchanged status echoes do not repeat that recovery fanout.

## Rationale and limits

The defect combined partial-coordinate Controller lookup/retention/diff with a Gateway first-match
lookup that ignored controllerName. Resource-local ownership avoids adding class-arrival or session
identity invalidation to already-compiled route units. The previous [delegation collision guard](../../routing/regression-guards/delegation-parentref-gateway-name-collision.md)
remains valid and is extended by controller ownership, not replaced.

Snapshot preservation and concurrent-writer safety are separate requirements: Kubernetes Merge
Patch replaces the parents array. The shared persistence path now conditions that replacement on
the raw input resourceVersion, rejecting a snapshot made stale by another writer. See the
[snapshot concurrency guard](kubernetes-status-snapshot-concurrency.md) for the correction,
coverage and recovery limits; ownership retention above remains necessary for fresh snapshots.

## Verification and re-evaluation

Historical targeted verification on 2026-09-20 (before class-scoped projection):

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

The 2026-09-21 class-scoped change passed 11 ownership tests, including the real
HTTPRoute handler/processor local-versus-persisted split and Gateway publication
recovery through the existing delayed queue. The encompassing processor suite
passed 819 tests. These tests are in-process, not live Kubernetes verification.
Final workspace validation and fresh independent review passed; see
[deployment observations](deployment-partitioned-observations.md) for results and limits.

## Resolution references

- `route-parent-status-canonical-identity`: canonical identity and configured ownership corrected;
  concurrent status persistence is covered by the separate guard above.
