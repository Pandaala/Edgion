---
name: controller-06-gatewayclass-egc-no-gateway-requeue
description: Separate GatewayClass ownership invalidation from parametersRef status propagation and data-plane listener reconstruction.
knowledge_type: intentional-design
---

# Class ownership requeues do not propagate parameter status or rebuild listeners

## Current decision

GatewayClass publication and unpublication requeue referencing raw Gateways for
**ownership admission and withdrawal**. The historical blanket statement that
GatewayClass-to-Gateway requeues are a no-op is superseded: Kubernetes Gateway
eligibility now reads the class owner and published incarnation.

The retained boundary is narrower: a Gateway requeue does not propagate
GatewayClass `Accepted` or EGC parameter errors into Gateway conditions, and does
not reconstruct live data-plane listeners. EGC has no direct Gateway requeue.
Its existing EGC-to-GatewayClass notification remains relevant to parametersRef
validation and may indirectly trigger an ownership recheck.

## Ownership mechanism

`GatewayClassOwnership` pairs the configured controller name with the selected
class in Kubernetes. `GatewayClassHandler::filter` accepts only that pair.
`GatewayHandler::filter` additionally requires a matching raw class owner and a
nonempty raw UID equal to the class already published by this deployment.
It does not compare generation or require `Accepted=True`.

Raw ownership alone can authorize a briefly owned incarnation which disappears
before its class worker ever publishes it. The published-UID requirement closes
that coalescing hole. Losing eligibility uses the processor's normal deletion
path to clear refs, listener registrations, cached Gateway and attachment proof.

`GatewayClassHandler::{after_publish,after_unpublish}` scan Gateway `raw_keys`,
select matching `gatewayClassName`, and call `ctx.requeue` after the cache mutation.
Scanning processed keys would omit previously rejected Gateways and break
missing-class recovery. The existing single queue/worker and dirty-key behavior
handle unpublication while a Gateway is already being processed. No API IO,
timer, second queue, dependency map or retry loop is added to handlers.

FileSystem/etcd preserve controller-name-only GatewayClass filtering and unscoped
Gateway handling. In Kubernetes, disabling the GatewayClass watcher fails closed
for Gateway processing. Independent deployments need distinct classes; replicas
sharing a class and controller identity need the same election domain and
configuration view. This is not fencing for arbitrarily overlapping deployments.

## Retained rationale

The data plane consumes GatewayClass and EGC independently through conf_sync.
Runtime policies with hot reload publish their own immutable snapshots; fields
frozen during listener construction still need their explicit restart/reload
contract. Controller ownership requeues cannot implement listener reconstruction.

Parameter errors remain visible on GatewayClass rather than copied into Gateway
conditions. Changing that propagation policy is a separate feature, not a
consequence of adding an ownership dependency.

## Evidence and re-evaluation

Implementation: `handlers/{gateway,gateway_class}.rs` and the contextual
eligibility/deletion path in `resource_processor/processor.rs`.
Controller-local coverage: `edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/mod_test/gateway_ownership_tests.rs` checks
missing/deleted/recreated classes, independent owners/classes, namespace filtering,
unpublished raw incarnations, UID/generation/Accepted boundaries, in-flight
unpublication, cleanup and repeated status echoes. These tests do not start a
Kubernetes server or prove convergence for overlapping independently elected writers.

Verified on 2026-09-21 for this ownership correction: 796 resource-processor tests
(including seven ownership/publication regressions), 20 Kubernetes resource-controller
tests, and `cargo check -p edgion-controller --all-targets` passed. Formatting,
agent-doc validation and `git diff --check` passed. Independent read-only final review
reached LGTM. These results do not include a live Kubernetes deployment.

Re-evaluate when class ownership, cache publication order, raw-key enumeration,
workqueue dirty handling or Gateway parameter-status propagation changes.

## Resolution references

- `controller-06` (2026-05-29): historical rejection of parameter-only Gateway
  requeues; its no-ownership-dependency premise is now superseded.
- `controller-07` (2026-06-01): EGC-to-GatewayClass parameter revalidation.
- `CAND-P2-1`: Gateway ownership and class lifecycle invalidation correction.

Related: [requeue inventory](../../../01-architecture/01-controller/06-requeue-mechanism.md),
[status concurrency](../../status/regression-guards/kubernetes-status-snapshot-concurrency.md),
[request-body hot reload](../../http-body/mandatory-design/request-body-policy-egc-hot-reload.md).
