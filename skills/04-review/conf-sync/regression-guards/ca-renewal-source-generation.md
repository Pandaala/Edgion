---
name: ca-renewal-source-generation
description: Review conf_sync CA renewal against concurrent root deletion and replacement.
knowledge_type: regression-guards
---

# CA renewal binds its source Secret generation

## Conclusion and scope

Controller auto-CA renewal must not restore a root removed for emergency re-key
or overwrite a concurrently installed root. `secret_io::renew_ca` reads the source
Secret, retains its UID and resourceVersion, renews with the same key, and submits
a conditional Merge patch of `ca.crt` and `ca.key`. Missing source metadata fails
closed. Kubernetes rejects missing or changed sources; `rotation::rotate` propagates
the failure before writing leaves or installing a server identity.

## Rationale and evidence

ISSUE-007 in `run-e6cbd068` identified an unconditional forced Apply that could
recreate a deleted CA using its old private key. Leadership generation checks do
not detect operator Secret deletion. The next reconcile would adopt the restored
root, defeating the documented emergency re-key operation.

The [lifecycle architecture](../../../01-architecture/03-controller-gateway-link/00-overview.md)
allows eventual per-replica convergence, not persistent restoration of old trust.
The [Kubernetes API contract](https://kubernetes.io/docs/reference/using-api/api-concepts/#updates-to-existing-resources)
supports resourceVersion-conditioned PATCH and distinguishes SSA creation from
non-SSA patches. Merge preserves unrelated fields and does not enforce SSA field
ownership conflicts; remaining leaf SSA writes still use force.

## Alternatives and limits

A pre-write GET alone leaves a check/write race. Removing SSA force alone can
permanently block renewal after POST creation or kubectl edits. Bootstrap still
uses POST-and-adopt. Renewal failure uses existing worker error/backoff handling,
without retrying the stale candidate. This is not a transaction across CA, leaf
and replica identities; deletion after a successful root write still relies on
ordinary convergence. ARCH-130's local publication guard has a different scope.

## Verification and re-evaluation

Client-transport tests check Merge media type, exact UID/resourceVersion, same-key
renewal, missing metadata, and propagation of 404/409/422/500 responses. These are
wire-contract tests, not proof of Kubernetes storage or fleet mTLS convergence.
`cargo test -p edgion-controller --lib services::conf_sync_ca` passed all 38 tests,
including rotation-level rejection before leaf publication.

A separate disposable-namespace check against local OrbStack Kubernetes
v1.35.6+orb1 verified the actual Merge API contract: normal renewal of POST-owned
fields preserved the key and unrelated data; concurrent modification rejected a
stale resourceVersion; deletion returned NotFound without recreation; recreation
with a different UID and an isolated UID mismatch both rejected the old write.
The namespace was deleted afterward. This used synthetic Secret data, not a
running Controller or a fleet re-key; full mTLS convergence was not rerun.

Independent review returned LGTM. Workspace check/clippy, formatting, agent-doc
validation, SSA force, metrics inventory, Pingora source policy, Gateway API
transform contract and unit-test layout checks passed. Clippy reported existing
warnings outside this change.

The cluster certificate harness P2.5 retains its same-SPKI/key, extended validity,
leaf-root consistency, conflict-free renewal and Gateway readiness assertions.
It no longer requires a root `managedFields` Apply entry because renewal uses
Merge. P2.9 still checks forced-SSA ownership takeover for the leaf Secret.
Re-evaluate if write mechanisms, retry policy, bootstrap ownership or root/leaf
publication ordering change. The re-key runbook remains the fleet-level check.

## Resolution references

- ISSUE-007-P1-ca-renewal-restores-revoked-root: confirmed defect; conditional
  renewal replaces unconditional root Apply.
