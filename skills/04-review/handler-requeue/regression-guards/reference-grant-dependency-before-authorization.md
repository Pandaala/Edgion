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
backendRefs; delegation children authorize their own backends as their own routes. HTTPRoute RequestMirror already
followed the same rule and now uses the shared ordering helper. TCPRoute,
TLSRoute, and UDPRoute register their complete direct backend dependency sets
before authorization. Gateway, Service, EdgionTls, EdgionAcme, and shared
credential resolution also register their applicable dependencies before grant
reads. Namespaced EdgionPlugins ConfigMap CA references follow the same ordering.
The two outbound TLS fields of cluster-scoped EdgionGatewayConfig instead use
administrative material selection: they register Secret/ConfigMap recovery
dependencies without a source namespace, Grant read, or Grant dependency. See
the [current EGC authority boundary](../../../01-architecture/05-resources/03-edgion-gateway-config.md).

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

The same historical audit found that grant-gated ConfigMap CA resolution registered only
the material dependency, and Service DNS resolver authorization had no
cross-namespace dependency lifecycle. Both could miss grant revocation. The
correction registered the owning EdgionGatewayConfig or EdgionPlugins before its
grant read. The later EGC reference migration replaced that former grant-gated
path with administrative selection; the ordering correction remains current for
namespaced EdgionPlugins. Service clears its old set at parse entry,
registers before the DNS grant read, and clears on delete; Gateway uses the same
DNS helper within its existing lifecycle.

`read_after_registering_cross_ns_dependency` now owns the ordering. HTTPRoute
and GRPCRoute clear once at parse entry, use the helper for every direct grant
read. A delegation child is reconciled as its own route, so its backend
authorization registers the child before reading. The later collection
pass is idempotent and does not clear dependencies.

Native HTTP/GRPC/TCP/TLS/UDP backendRefs now reach that helper only through
`refs::route_fields` authored terminal loans and Controller `route_backend_refs`.
Canonical source/target validation precedes dependency creation. The target
namespace is registered before reading even the validation-enable flag. L4 installs
its complete valid namespace set before the first authorization read. Effective
status/AI views and owned receipts cannot mint Grant queries; denied terminals
retain their dependencies but do not cause status/AI target reads.

Under the cascading-match delegation model no field is cloned between routes:
the child is both the authorization source and the requeue owner of its own
backends. (History: the removed Controller flattening authorized child fields before cloning them into the parent; that path no longer exists.)
Valid receipts are collected before later rule filtering. Invalid source/target
zero-read assertions concern discovery from the new descriptor; necessary cleanup
and requeue enumeration of existing legal old dependencies still runs.

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
`CrossNsRefManager`. (Historical: flattening-era delegation tests confirmed that
denied siblings kept `ref_denied` when copied; with no copies that check is moot.) Namespaced ConfigMap CA and Service DNS tests confirm exact owner
registration, replacement cleanup, and delete cleanup. Current EGC handler tests
instead confirm administrative material selection, canonical cluster owner
recovery dependencies, and no Grant edges; grants do not alter its TLS material
selection. The older EGC grant-ordering verification is historical evidence only.

Re-evaluate when adding a new `ReferenceGrantStore` reader, changing event
dispatch granularity, replacing `CrossNsRefManager`, or adding a new derived
authorization result that is published before its dependency graph is complete.

## Resolution references

- `reference-grant-dependency-registration-race`: confirmed runtime authorization-revocation race; corrected for HTTPRoute, GRPCRoute, grant-gated ConfigMap CA consumers, and Service/Gateway DNS resolver references after auditing current authorization readers.
