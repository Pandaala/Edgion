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

Canonical authority: the vendored Gateway API v1.5.0 CRDs in
`config/crd/gateway-api/`, and [GEP-713](https://gateway-api.sigs.k8s.io/geps/gep-713/).

The policy ancestor contract is verbatim in
`config/crd/gateway-api/gateway-api-experimental-v1.5.0.yaml` (standard channel
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

Edgion's native path complies. `status_ownership::native_status_projection`
matches entries through `own_entry`, which compares `controllerName` alone, and
`continue`s past every foreign entry before inspecting it; foreign entries from
the observed snapshot are pushed through unchanged.

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
envelope just read, so foreign entries are carried forward each round except
where the private envelope's own pruning applies (below). The only paths that
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

**Relevant checks: none.** No repository guard, lint, or test enforces
foreign-entry preservation for either status family. Compliance is established
by review against this page and by upstream conformance for the native lists.

## Relationship to the private envelope

Descriptive only; this section states no requirement.

`EdgionPlugins`, `EdgionStreamPlugins`, `EdgionBackend` and `LinkSys` carry a
private `controllers[]` envelope, capped at 32. It is Kubernetes-only: it exists
solely when `ControllerRuntime::status_identity()` is `Some`, and FileSystem and
etcd discard an inherited envelope and recompute local diagnostics
(`processor.rs`, `has_non_native_status` → `replace_status(Null)`), because one
configuration source there serves one Controller. Multi-writer attribution is a
Kubernetes-mode concern and should not be ported to native modes.

Unlike the native lists, the envelope keys entries on `controllerName` plus
`gatewayClassName` (`parse_envelope`'s duplicate check and `merge_status`'s
own-entry replacement use that pair; `gatewayClassUid` proves incarnation and is
read only by `project_status`), and `proven_obsolete` prunes another
deployment's entry on authoritative class deletion or UID/controller mismatch.
That is the opposite of the upstream shape. It is not a violation of this page —
these are Edgion's own CRDs, outside the Gateway API contract — and it is not a
defect: it is the contract recorded, with verification, in
[deployment-partitioned observations](../regression-guards/deployment-partitioned-observations.md),
**which governs the envelope.** This page governs the native lists.

The open question is whether the divergence should persist. The premise that
justified the class key — independent deployments possibly sharing one
`controllerName` — was retired by
[GatewayClass reporting and publication](../regression-guards/gatewayclass-reporting-and-publication.md),
which now requires distinct `controllerName` values per deployment; the
partitioning was left unchanged when that premise was withdrawn. Aligning the
envelope with the upstream shape is proposed, not decided, in the task linked
below. Until that lands, a reviewer should treat the envelope's pruning as the
recorded and verified contract, not as a finding.

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

- Related proposal: [multiple GatewayClasses per Kubernetes Controller deployment](../../../../tasks/todo/kubernetes-gateway-class-allow-list.md)
  — would align the private envelope with the upstream shape and remove the
  vestigial class key. Not a prerequisite for this page's native-list rule.
