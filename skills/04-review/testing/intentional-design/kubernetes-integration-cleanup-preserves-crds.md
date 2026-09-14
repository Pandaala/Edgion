---
name: kubernetes-integration-cleanup-preserves-crds
description: Use when reviewing Kubernetes integration cleanup scope or proposals to delete CRDs from the test harness.
knowledge_type: intentional-design
---

# Kubernetes integration cleanup preserves shared CRDs

## Conclusion and scope

`edgion-tests/k8s/scripts/cleanup.sh` never deletes CRDs. It removes only the
namespaces and exact cluster-scoped objects recorded in the integration ownership
inventory, plus optional local images and generated artifacts. CRDs are
installation-scoped infrastructure that may be shared by multiple developers or
test runs, so their lifecycle belongs to a separate cluster installation or
maintenance workflow.

The cleanup CLI intentionally has no `--with-crds` option. Supplying that removed
option is an error rather than a compatibility alias.

## Rationale and evidence

The earlier explicit `--with-crds` path could delete Edgion and Gateway API CRDs
from whichever Kubernetes context was selected. Even with correct object-verification
ordering, an accidental invocation on a shared test cluster could disrupt unrelated
users and remove custom resources outside the current run's ownership inventory.

The integration runner itself never passed `--with-crds`, and CRD installation
already comes from the repository-wide `config/crd` bundle rather than the
run-owned Kubernetes fixture directory. Removing CRD deletion makes the cleanup
boundary match that ownership model.

## Alternatives and limits

Keeping a guarded `--with-crds` option was rejected because a command-line switch is
too weak a boundary for shared, cluster-wide infrastructure. Ordering CRD deletion
after owned-resource verification prevents discovery errors but does not prevent
collateral deletion of resources owned by other users.

The accepted cost is that stale or obsolete CRDs require an explicit external
maintenance action. This cleanup still deletes owned CRD-backed objects such as
`EdgionGatewayConfig`; it preserves the CRD definitions, not every custom resource.
It does not manage CRD upgrades, version compatibility, or cluster provisioning.

## Verification and re-evaluation

`bash edgion-tests/k8s/scripts/common/tests/cleanup_test.sh` covers an owned
`EdgionGatewayConfig`, retained-object failure, ownership bounds, and rejection of
the removed `--with-crds` option. The test also rejects manifest-wide `kubectl
delete -f` or `-k` calls from cleanup.

Re-evaluate only if Edgion gains an isolated, single-owner ephemeral cluster workflow
with an independently authorized cluster teardown command. Do not broaden this
shared cleanup script for that use case.

## Resolution references

- `k8s-cleanup-crd-verification-order` — the original ordering defect was superseded
  by removing CRD deletion from integration cleanup on 2026-09-14.
