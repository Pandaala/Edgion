---
name: kubernetes-status-snapshot-concurrency
description: Review shared Kubernetes status persistence, raw resourceVersion preconditions, foreign ownership, and conflict recovery limits.
knowledge_type: regression-guards
---

# Kubernetes status writes use their raw snapshot version

## Conclusion and scope

`ResourceController<K>` captures the input `metadata.resourceVersion` before processing
in both bootstrap (`run_init_apply_once`) and runtime (`spawn_worker`). Its one generic
`persist_k8s_status<K>` sends JSON Merge Patch containing that raw version and computed
status. A missing/empty version fails locally; it never causes an unconditional request.
An intervening object update causes a 409 rather than silently accepting stale state.

This covers all 17 current status-computing handlers: HTTPRoute, GRPCRoute, TCPRoute,
TLSRoute, UDPRoute, EdgionTls, BackendTLSPolicy, EdgionBackendTrafficPolicy, Gateway,
GatewayClass, EdgionGatewayConfig, EdgionPlugins, EdgionStreamPlugins, EdgionConfigData,
EdgionBackend, LinkSys and EdgionAcme. GatewayClass and EdgionGatewayConfig are
cluster-scoped. Future resources using the same controller inherit the guard.
Secret, ConfigMap, Service, EndpointSlice and ReferenceGrant do not compute status;
Namespace is a separate observer without status writes. FS/etcd persistence is outside
the Kubernetes version contract.

## Rationale and evidence

The original defect (`route-status-concurrent-writer-preservation`) was broader than
Routes: the shared function submitted `{status: ...}` without any precondition. Merge
Patch replaces arrays wholesale, so a foreign writer's successful update after snapshot
capture could be lost. The correction binds the whole PATCH atomically to that snapshot.
Versions remain opaque; processed metadata, sync versions, or later GET results are not
valid replacements for the raw input version.

Canonical contract: [status management](../../../01-architecture/01-controller/10-status-management.md).
Implementation: `edgion-controller/src/conf_mgr/conf_center/kubernetes/resource_controller.rs`
(`run_init_apply_once`, `spawn_worker`, `StatusWriteError`, `persist_k8s_status`).

Snapshot ownership is independently necessary. Route-family retention preserves foreign
parents. `EdgionBackendTrafficPolicyHandler::update_status` retains foreign ancestors
while rebuilding its own entries, matching BackendTLSPolicy; stale owned entries disappear
and unchanged owned conditions retain transition times. See [parent ownership](canonical-parent-status-ownership.md).

For both policy kinds, semantic status comparison must match canonical ancestor identity
and `controllerName` one-to-one, including multiplicity. Matching the first name/namespace
entry can compare an owned entry against a foreign entry for the same ancestor, falsely
report a change, and sustain a WATCH/PATCH loop. Shared `attachment_statuses_equal` in
`status_diff.rs` serves parents and ancestors with the same identity/ownership rules.
The semantic comparison applies across Kubernetes, FileSystem and etcd; the versioned
PATCH precondition is specific to Kubernetes.

## Alternatives and limits

- CAS prevents stale snapshots, not disagreement between fresh writers. ACME
  handler admission and service eligibility now share `acme_is_owned`, preventing
  foreign classes from writing competing handler conditions. This is separate
  from notification deduplication. Active-attempt ownership handoff and scheduler
  recovery remain outside this correction.
- One API attempt is made per changed status. 409 uses the existing warning/error-metric
  path, without immediate GET/retry or failure-triggered requeue. Later events, source LIST,
  dependency requeue or leadership acquisition can reconcile again. There is no bounded
  convergence guarantee; see [no retry](../../handler-requeue/intentional-design/status-write-no-retry-accepted.md).
- Explicit empty lists retain their Merge Patch clearing semantics. Omitting a field is
  not a tombstone; see [list serialization](../../handler-requeue/regression-guards/status-list-merge-patch-tombstone.md).
- ACME's two forced SSA service writers remain specialized. The generic version guard
  rejects stale phase/challenge rollbacks, but does not establish nonoverlapping field
  ownership; `last_failure_reason` remains an overlapping field outside this correction.
  Full-snapshot forced SSA would not by itself solve ownership.
- Version checking is not leader fencing: a demoted writer holding the current version
  can still pass it. The [AllServe gate decision](../../concurrency/intentional-design/leader-status-write-per-write-guard-vs-lifecycle-gate.md)
  remains a separate lifecycle tradeoff. Handlers still do no API IO, and the worker keeps
  inline status IO, one queue and one worker per kind.

## Verification and re-evaluation

Controller-local regression coverage is in
`edgion-controller/tests/unit/conf_mgr/conf_center/kubernetes/resource_controller_test/status_persistence_tests.rs`,
`status_diff.rs` and the EBTP handler's tests. The transport model covers real resource
types, endpoint scope, version conditions, foreign writes, stale failures and later
reconciliation; the kind inventory is compared with `ALL_RESOURCE_INFOS` to catch future
omissions. Both policy kinds have canonical identity, ownership and multiplicity coverage.
These are in-memory transport tests, not a live Kubernetes API-server reproduction.

Verified on 2026-09-21:

- Controller resource-controller tests: 20 passed, including 9 new persistence/lifecycle tests.
- Controller resource-processor tests: 787 passed, including policy identity and repeat-status guards.
- `cargo fmt --all` and `cargo fmt --all -- --check`: passed.
- `cargo check --workspace --all-targets`: passed.
- `cargo clippy --workspace --all-targets`: passed with two existing warnings in unchanged
  `edgion-tests` DSL gRPC tests and Gateway Kafka tests; no new warning in the changed code.
- Agent-doc validation, SSA force, metrics inventory, Pingora source policy and Gateway API
  transform contract checks: passed. `git diff --check`: passed.
- Independent review of the implementation, ownership/comparison amendment and knowledge
  found no remaining issues. Existing integration tests were not modified; no full-workspace
  test run or live Kubernetes validation is claimed.

Re-evaluate when adding a status writer/kind, changing raw-source ingestion, processing
metadata, resource scope/API versions, patch format, ownership/diff, or retry/leadership policy.

## Resolution references

- `route-status-concurrent-writer-preservation`: shared stale-snapshot status overwrite,
  EBTP foreign-ancestor retention and ownership-aware ancestor diff corrected and verified
  as described above. A later request for immediate same-generation Route conflict
  recovery was declined; the accepted no-retry limit is recorded in
  [status-write failure policy](../../handler-requeue/intentional-design/status-write-no-retry-accepted.md).
