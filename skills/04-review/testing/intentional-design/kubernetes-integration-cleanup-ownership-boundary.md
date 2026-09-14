---
name: kubernetes-integration-cleanup-ownership-boundary
description: Use when reviewing Kubernetes integration cleanup scope, concurrent runs, context safety, or shared infrastructure deletion.
knowledge_type: intentional-design
---

# Kubernetes integration cleanup requires run-scoped ownership

## Conclusion and scope

`edgion-tests/k8s/scripts/cleanup.sh` deletes cluster state only when the caller
provides a recorded `owned-cluster-keys.tsv` and an exact kubectl context. It never
infers ownership by scanning the fixture tree. Every kubectl deletion and
verification call is pinned to that context.

The fixed-name integration topology is single-writer per cluster. The runner holds
an atomically created Kubernetes Lease for its complete lifecycle and refuses to
start while another holder exists. Cleanup removes test-owned business namespaces,
`GatewayClass` objects, and `EdgionGatewayConfig` objects. It never deletes CRDs,
the installation namespaces `edgion-system` and `dragonfly-system`, Controller
ClusterRole/ClusterRoleBinding objects, or local Docker images.

Generated certificates and manifests live under
`tmp/k8s_integration/<run-id>/generated`. Cleanup derives that exact directory from
the ownership file's run directory and does not accept a caller-selected recursive
deletion target.

## Rationale and evidence

The earlier standalone cleanup synthesized an ownership list from every repository
fixture. That proved only that a name was allowed, not that the live object belonged
to the caller. On a shared or incorrectly selected cluster it could delete whole
generic namespaces such as `app` and `backend`, cluster-wide RBAC named
`edgion-controller`, and shared Gateway configuration.

An explicit `--with-crds` path created a still broader cluster-wide risk, while
`--with-images` ran a global Docker prune and removed old matching images from the
host daemon. A fixed repository `generated` directory also allowed concurrent local
runs to remove each other's runtime secrets. These surfaces were removed rather
than guarded by compatibility aliases.

The runner's former absence probes were not an atomic claim: two concurrent starts
could both observe the fixed names as absent before either applied resources. The
Lease closes that check-then-act race for cooperating integration runs. Release
checks `holderIdentity` and never deletes another run's Lease.

## Alternatives and limits

A warning, interactive prompt, optional context environment variable, or dangerous
all-fixtures flag is too weak for shared infrastructure. A separate installation or
administrator maintenance workflow owns CRD, installation RBAC/namespace, stale
Lease, and image cleanup.

The Lease deliberately has no automatic expiry or stealing behavior: a crashed
process may leave a stale Lease, but silently stealing it could overlap an active
run after a transient client failure. An operator must inspect and remove a proven
stale Lease through an external maintenance action.

The test topology still uses fixed object names and is not a multi-tenant parallel
test system. The Lease serializes it; supporting parallel runs would require
run-qualified namespaces, cluster object names, routing identities, and ports.

## Verification and re-evaluation

Focused Shell regressions cover required ownership/context arguments, context
pinning, installation-resource exclusions, run-local generated cleanup, removed
CRD/image options, retained resources, API failures, atomic Lease conflict, missing
Lease release, and holder mismatch.

On 2026-09-14, every script under
`edgion-tests/k8s/scripts/common/tests/*.sh` passed. Repository verification also
passed `cargo fmt --all`, `cargo check --workspace --all-targets`,
`cargo clippy --workspace --all-targets` (one unchanged redundant-async warning),
and the agent-doc, SSA-force, metrics-inventory, and Pingora source-policy guards.
No live Kubernetes run or `cargo test --all` was performed for this cleanup change.

Re-evaluate only if the topology becomes fully run-qualified or Edgion gains an
isolated single-owner ephemeral cluster teardown workflow. Do not broaden the shared
cleanup script for those cases.

## Resolution references

- `k8s-cleanup-crd-verification-order` — the original CRD-ordering defect led to the
  broader cleanup ownership review and removal of CRD deletion on 2026-09-14.
