---
name: reference-grant-dependency-before-authorization
description: Use when reviewing Controller handlers that read ReferenceGrant authorization and rely on CrossNsRefManager for change revalidation.
knowledge_type: regression-guards
finding_id: reference-grant-dependency-registration-race
---

# Register ReferenceGrant dependencies before authorization reads

## Conclusion and scope

Controller handlers that publish a result derived from `ReferenceGrantStore` must
make their `CrossNsRefManager` dependency visible before reading that store. An
apply reconciliation clears the owner's previous dependency set once, then only
extends it until all authorization reads and derived publication are complete.

HTTPRoute and GRPCRoute follow this ordering for ordinary cross-namespace
backendRefs and flattened delegated backends. HTTPRoute RequestMirror already
followed the same rule and now uses the shared ordering helper. TCPRoute,
TLSRoute, and UDPRoute register their complete direct backend dependency sets
before authorization. Gateway, Service, EdgionTls, EdgionAcme, and shared
credential resolution also register their applicable dependencies before grant
reads. The grant-gated ConfigMap CA path covers EdgionGatewayConfig and
EdgionPlugins while preserving the cluster-scoped EdgionGatewayConfig identity.

## Rationale and evidence

ReferenceGrant kinds and Route kinds use independent serial workqueues. On grant
deletion, `ReferenceGrantHandler::on_delete` removes the grant from
`ReferenceGrantStore` and then dispatches an event. The listener snapshots
dependents from `CrossNsRefManager`; it cannot requeue an owner that has not yet
registered.

Previously, HTTPRoute and GRPCRoute ordinary backendRefs read the grant before
their later `record_cross_ns_refs` pass. GRPCRoute also cleared its old set in
that late pass. A deletion between the successful read and registration could
therefore miss the Route, after which the Route could publish stale
`ref_denied = None` and `ResolvedRefs=True`.

The same audit found that grant-gated ConfigMap CA resolution registered only
the material dependency, and Service DNS resolver authorization had no
cross-namespace dependency lifecycle. Both could miss grant revocation. The
ConfigMap fetch path now registers the owning EdgionGatewayConfig or
EdgionPlugins before its grant read. Service clears its old set at parse entry,
registers before the DNS grant read, and clears on delete; Gateway uses the same
DNS helper within its existing lifecycle.

`read_after_registering_cross_ns_dependency` now owns the ordering. HTTPRoute
and GRPCRoute clear once at parse entry, use the helper for every direct grant
read, and pass the root owner into delegation flattening so child-declared
backend authorization registers the parent before reading. The later collection
pass is idempotent and does not clear dependencies.

The workqueue discipline in the repository-root `AGENTS.md` remains authoritative:
grant changes requeue the existing per-kind worker; this correction adds no
queue, wait, polling path, or handler IO.

## Alternatives and limits

Registering only after authorization does not close the event window. A one-time
post-initialization revalidation cannot repair a runtime deletion interleaving.
Moving authorization to the Gateway would duplicate the Controller-owned store
and change the established transport boundary.

The dependency key is the target namespace because a ReferenceGrant event is
dispatched by affected namespace. Authorization still uses the exact declaring
namespace, group, kind, target group/kind, and optional target name. Registration
does not grant access and remains required when validation is disabled so a
later policy/configuration transition retains the complete dependency graph.

This rule guarantees that a completed grant-change event cannot be missed by an
in-flight authorization read. It does not retroactively cancel already-started
Gateway requests or establish an immediate data-plane revocation fence.

## Verification and re-evaluation

The deterministic unit test
`dependency_is_visible_before_authorization_read` executes an observation inside
the protected read closure and confirms that the owner is already visible in
`CrossNsRefManager`. HTTPRoute and GRPCRoute delegation tests confirm that
denied authored siblings are copied into the effective rules with `ref_denied`
intact. ConfigMap CA and Service DNS tests confirm exact owner registration,
replacement cleanup, and delete cleanup, including the cluster-scoped
EdgionGatewayConfig owner.

Re-evaluate when adding a new `ReferenceGrantStore` reader, changing event
dispatch granularity, replacing `CrossNsRefManager`, or adding a new derived
authorization result that is published before its dependency graph is complete.

## Resolution references

- `reference-grant-dependency-registration-race`: confirmed runtime authorization-revocation race; corrected for HTTPRoute, GRPCRoute, grant-gated ConfigMap CA consumers, and Service/Gateway DNS resolver references after auditing current authorization readers.
