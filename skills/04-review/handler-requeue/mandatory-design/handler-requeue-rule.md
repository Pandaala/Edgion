---
name: handler-requeue-rule
description: Use when reviewing ProcessorHandler implementations; Gateway snapshot consumers must maintain dependency edges with the correct registration, publication, and deletion ordering.
knowledge_type: mandatory-design
---

# Handler Requeue Rule

> Confirmed 2026-03-17 based on actual bug fix in `edgion_tls.rs`.

## Rule

**Any `ProcessorHandler` that calls `lookup_gateway()` in `parse()` MUST also:**

1. Maintain `GatewayRouteIndex` dependencies on apply: ordinary Routes finalize their edge set in `on_change`; EdgionTls registers in `parse` before its proof lookup
2. Implement cleanup in `on_delete()` with `remove_from_gateway_route_index()`

Without this, when the handler processes a resource before its Gateway exists,
`resolved_listeners` (or effective hostnames) will be permanently stale because
Gateway's `after_publish()` relies on `gateway_route_index` to find and requeue
affected resources.

Snapshot-dependent attachments also require registration before the first
Gateway read, so publication cannot miss a dependency while that read is in
flight. HTTPRoute, GRPCRoute, TCPRoute, UDPRoute and TLSRoute add new edges in
parse and keep exact replacement in `on_change`: dropped parents must be
requeued only after `update_status`
updates their attachment counts. EdgionTls registers before its proof lookup.

For `controller-route-attachment-gateway-ordering-race`, the regression
`http_routes_converge_after_concurrent_gateway_publication` publishes a Gateway
between an HTTPRoute's empty listener read and its final index update. Both
that route and an earlier sibling must acquire the listener through queued work
after the sources stop changing. The test checks the dependency at the first
Gateway source read, the transport parent status (`Accepted=True`), and the
Gateway's attached route count. FileSystem rejects an HTTPRoute with no
namespace at ingestion so the queued key identifies the original source file.
Reprocessing an unchanged Gateway alone cannot
repair a missed notification because listener inputs are compared on publication.

For `controller-grpc-tcp-udp-route-gateway-dependency-order`, the GRPC, TCP
and UDP variants of `routes_converge_after_concurrent_gateway_publication`
exercise this ordering for namespaced sources. Only Gateway parentRefs enter
the index (including defaulted group/kind); a gRPC delegation parent does not
subscribe to a same-name Gateway. TCP and UDP reject multi-rule sources before
listener resolution; adding an edge for an unpublished first generation would
leave no cached route for deletion to clean up. On reparenting, the old Gateway
remains indexed during parse and is requeued after its attached count drops.
The three new first-read assertions failed on the previous handlers; all nine
Gateway route-refresh tests passed with early registration.

GRPCRoute, TCPRoute and UDPRoute skip Gateway-dependent acceptance errors for
an invalid route reference source. Such a route cannot attach. Its validation
diagnostic must remain available even if the Gateway appears after the status
read and the route never receives another notification. The invalid-owner
status regression checks the same source error with missing and present parents.

FileSystem and direct etcd route keys with an omitted namespace have a separate
[requeue-key issue](../../../../tasks/todo/controller-offline-route-namespace-requeue-key.md).

## Affected handlers

| Handler | lookup_gateway() in parse() | gateway_route_index in on_change() |
|---------|:--:|:--:|
| `http_route.rs` | Yes (via hostname_resolution) | Yes |
| `grpc_route.rs` | Yes (via hostname_resolution) | Yes |
| `tls_route.rs` | Yes | Yes |
| `edgion_tls.rs` | Yes | Registered in parse before proof lookup; on_change has no index work |
| `tcp_route.rs` | Yes (via resolve_route_listener_attachments) | Yes |
| `udp_route.rs` | Yes (via resolve_route_listener_attachments) | Yes |

## How to verify

When adding a new handler that references Gateway:

1. Check if `parse()` calls `lookup_gateway()` or `route_utils::lookup_gateway()`
2. If yes, verify apply-side dependency registration and its snapshot-read ordering; ordinary Routes finalize with `update_gateway_route_index()` in `on_change`
3. Ensure `on_delete()` calls `remove_from_gateway_route_index()`
4. Add a `BothAbsentParentRef` integration test that includes a Gateway requeue cycle

## Background

During init phase, resources are processed by independent per-kind Workqueues
in parallel. There is no guaranteed ordering between resource types. After init,
three revalidation mechanisms run:

- `trigger_full_cross_ns_revalidation()` — requeues resources with cross-ns refs
- `trigger_gateway_secret_revalidation()` — requeues all Gateways for TLS cert re-check
- `trigger_gateway_route_revalidation()` — requeues all routes in gateway_route_index

The third was added (2026-03-17) to cover the "Gateway processed before Route
registers in gateway_route_index" timing gap. Without it, Gateway's publication hook
detects changed Listener routing inputs but `get_routes_for_gateway()`
returns empty because no routes have registered yet. The one-time post-init
requeue ensures all registered routes get reprocessed with the Gateway in cache.

See also: `skills/01-architecture/01-controller/06-requeue-mechanism.md` for the full map.

## Also applies to Secret references

Handlers that call `get_secret()` in `parse()` MUST register with
`secret_ref_manager` so that `SecretHandler.on_change()` can requeue them
when the Secret arrives. All current handlers already do this correctly.
