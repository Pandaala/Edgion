---
name: foreign-status-entry-preservation
description: Use when reviewing whether a Gateway API status list may be keyed on something other than controllerName, or whether an implementation may evict, truncate, or garbage-collect another controller's entry.
knowledge_type: mandatory-design
---

# Foreign entries in Gateway API status lists are preserved, never reclaimed

## Conclusion and scope

In a Gateway API status list shared by several controllers, an implementation
identifies its own entries by `controllerName`, writes only those, and preserves
every other entry — including entries it can prove are stale. Staleness is not
authority to delete: the specification defines no reclamation of another
controller's observation, and capacity pressure does not create one.

Scope: Route `status.parents` (HTTPRoute, GRPCRoute, TCPRoute, TLSRoute,
UDPRoute), policy `status.ancestors` (`BackendTLSPolicy`,
`EdgionBackendTrafficPolicy`), and `EdgionTls` parents. The implementation-side
guard for these lists is
[canonical parent status ownership](../regression-guards/canonical-parent-status-ownership.md);
this page records the upstream contract that guard is measured against, not a
second copy of its retention rules.

**Out of scope: Edgion's private `controllers[]` envelope.** The boundary is
shape and writer, not CRD ownership — `EdgionTls` is an Edgion CRD and is in
scope, because it adopts the Gateway API `ParentReference` plus `controllerName`
parent-status shape and is written by the same `native_status_projection` path as
the five Routes. The envelope instead defines its own schema with its own key and
its own writer, so this contract does not reach it. See "Relationship to the
private envelope" below — that section is descriptive, and deliberately states
no MUST.

## Rationale and evidence

Canonical authority: the vendored Gateway API v1.6.2 CRDs in
`config/crd/gateway-api/`, and [GEP-713](https://gateway-api.sigs.k8s.io/geps/gep-713/).

The policy ancestor contract is verbatim in
`config/crd/gateway-api/gateway-api-experimental-v1.6.2.yaml` (standard channel
carries the same text): implementations MUST use the ControllerName field to
uniquely identify the entries in the list they are responsible for, and the list
MUST be treated as a map with a composite key made up of the AncestorRef and
ControllerName fields combined. **The composite key contains no GatewayClass.**
Route `status.parents` carries the parallel "MUST ONLY populate ... responsible
for" language.

Overflow is a terminal signal, not a trigger to reclaim. With a maximum of 16
ancestors, a full list means implementations MUST NOT add further entries and
MUST instead consider the policy unimplementable and signal that on related
resources. Route parents bound at 32.

Preservation and capacity enforcement are separate obligations. The canonical
projection preserves foreign observed entries; same-controller entries also
require current attachment ownership proof before replacement. ARCH-106 adds
final policy capacity composition after that proof: protected history consumes
the 16-entry budget, existing owned keys reserve slots first, and new attachments
without slots are refused in both diagnostics and serving. A protected input
already exceeding 16 suppresses persistence instead of truncating history or
writing an oversized list. See the [shared policy capacity contract](../../../01-architecture/05-resources/12-backend-tls-policy.md#final-ancestor-capacity-and-serving-admission-arch-106).
The earlier preservation-only compliance statement did not establish overflow
behavior and is superseded by these separate checks.

**Externally sourced, not corroborable in this repository:** upstream
conformance reinforces preservation directly. `kubernetes-sigs/gateway-api` PRs
5138 and 5206 seed a foreign entry under a reserved `StaleControllerName` value,
bump the generation, wait for the implementation under test to refresh its own
entry, and require the seeded entry to survive unchanged. This repository does
not vendor the conformance suite, so that detail rests on the upstream PRs. The
normative contract above does not depend on it and is verifiable in-repo.

A spec update does not clear status. All nine Edgion CRDs under
`config/crd/edgion-crd/` enable the `status` subresource, so `.status` submitted
through the main endpoint is ignored and stripped by the API server; an `apply`
that omits status is not a deletion instruction. The reconcile that follows a
spec change recomputes only the writer's own entry and merges it into the
envelope just read, so foreign entries are carried forward each round. The only paths that
clear an entry are deleting and recreating the resource, or an explicit
`kubectl patch <kind> <name> --subresource=status`.

## Alternatives and limits

Rejected: evicting or truncating a foreign entry to make room, and reading a
capacity limit as permission to reclaim. Both break the contract above.

Consequence of a violation: an implementation that deletes another controller's
entry destroys status that controller is responsible for and that no reconcile of
its own will restore, because the owner is not watching for the deletion.

Accepted cost of not reclaiming: an entry left by a permanently removed or
renamed deployment persists until the resource is recreated or an operator
patches the status subresource. Deployment identity is stable in practice —
`GatewayClass.spec.controllerName` is immutable upstream, and Edgion resolves an
explicitly configured `conf_center.controller_name` first, falling back to
`POD_NAMESPACE` plus `EDGION_CONTROLLER_DEPLOYMENT_NAME` only when it is omitted
(`config/conf_center/controller_identity.rs`) — so reaching a cap requires many
distinct deployments having observed the same resource.

Relevant regression coverage exists in `status_ownership.rs`,
`handlers/tests/parent_status_identity_test.rs` and `status_observation.rs` for
native ownership and private-envelope preservation/capacity. These code tests do
not establish a live Kubernetes or upstream conformance result by themselves.

## Relationship to the private envelope

Descriptive only; this section states no requirement.

`EdgionPlugins`, `EdgionStreamPlugins`, `EdgionBackend` and `LinkSys` carry a
private `controllers[]` envelope, capped at 32. It is Kubernetes-only: it exists
solely when `ControllerRuntime::status_identity()` is `Some`, and FileSystem and
etcd discard an inherited envelope and recompute local diagnostics
(`processor.rs`, `has_non_native_status` → `replace_status(Null)`), because one
configuration source there serves one Controller. Multi-writer attribution is a
Kubernetes-mode concern and should not be ported to native modes.

The private envelope keys entries only on `controllerName`. Duplicate detection,
projection and replacement share that key; there is no class anchor or
class-evidence pruning. Merge retains every foreign observation and its order
from the observed snapshot, including stale deployment entries. Typed ingest may
normalize optional empty fields before the snapshot; this is not a claim of
byte-for-byte retention of arbitrary API JSON. An existing owner can update at
32 entries; a new owner gets a warning and suppressed status persistence, without
blocking local configuration processing.

This alignment was explicitly authorized by `kubernetes-gateway-class-allow-list`.
It is a product choice for Edgion's CRDs, not a rule imposed by Gateway API on
this private schema. The previous class key and pruning contract are superseded.
[Deployment-partitioned observations](../regression-guards/deployment-partitioned-observations.md)
governs the envelope and records its verification. Distinct deployment identities
and the raw/published class UID proof for serving remain required.

## Verification and re-evaluation

No fresh test execution is claimed by this page. It records the upstream
specification as vendored in this repository, plus a 2026-09-23 reading of
`status_ownership.rs`, `status_observation.rs`, `status_pipeline.rs`,
`processor.rs`, `controller_status.rs`, `controller_identity.rs`,
`resource_controller.rs` and the committed CRDs.

Re-evaluate when Gateway API introduces an official stale-entry reclamation
mechanism, when a new partitioned status kind is added, when a status list gains
a writer outside the canonical pipeline, or when the private envelope's key
changes.

## Resolution references

- `kubernetes-gateway-class-allow-list`: removes the private envelope's class key
  and pruning; the native-list requirement remains independently authoritative.
