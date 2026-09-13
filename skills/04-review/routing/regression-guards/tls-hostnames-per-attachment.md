---
name: tls-hostnames-per-attachment
description: Review TLSRoute hostname projection, accepted attachment selection, and per-listener rebuilds.
knowledge_type: regression-guards
---

# TLSRoute hostname projection stays with its listener attachment

## Conclusion and scope

The Controller transports `ResolvedTlsListener` records in
`TLSRouteSpec.resolved_listeners`. Each record binds Gateway namespace/name,
logical listener name, physical `ListenerKey`, and the effective hostname list.
There is no route-global `effective_hostnames` field or Gateway fallback to raw
hostnames for unresolved routes. Missing, empty, or invalid route hostnames do
not produce catch-all routes. An unrestricted Listener preserves the route's
explicit names.

The reference is [Gateway API v1.6.2 TLSRoute](https://github.com/kubernetes-sigs/gateway-api/blob/v1.6.2/apis/v1/tlsroute_types.go),
which requires 1..1024 hostnames and intersection per attachment. This correction
does not upgrade the repository's complete v1.5.0 CRD bundle; installed schema
limits and broader API-version alignment require the coordinated version task.

## Rationale and evidence

`route_utils::resolve_tls_listener_hostnames` uses canonical parent resolution,
projects each listener independently, discards empty intersections, and replaces
the prior records. The Gateway unit filters these against accepted parentRefs
including port/section identity. `TlsRouteTable::from_units` admits hostname
buckets only from those accepted attachments on the table's own physical key.
Both TLS proxies pass only that key's exact logical parents to existing policy
admission, preventing another port's parent from supplying logical ownership.
Global TLSRoute validation errors also clear the attachment tracker: rejected
routes must not contribute to Gateway `attachedRoutes`, including after a
previous valid generation. Attachment status, projection, and counts must
recover together when explicit valid hostnames are restored.

Gateway refresh must preserve these associations too. `GatewayRouteIndex`
compares records containing each listener's name, optional hostname, resolved
physical key, raw port/protocol and allowedRoutes. Independently sorted hostname
and port arrays miss renames and hostname exchanges, leaving exact TLS parents
stale. Records exclude metadata, status, proof epochs and TLS credentials.
The Gateway handler detects changes and requeues dependents in `after_publish`,
after readers can observe the new cache value. `after_unpublish` clears the
record and requeues dependents before recreation; dependency edges remain.

TLSRoute pre-registers new parent edges before reading Gateway snapshots, using
the same index. Otherwise a concurrent Gateway publication can miss the new
dependency after the Route has read an older snapshot. Old edges remain until
the normal `on_change` replacement, after attachment status/counts have been
updated; only then are dropped parents requeued. The current TLS parse always
returns Continue. A future early Skip after registration must define cleanup.

Originally a route attached to a.example.com on port A and b.example.com on port B
put both names in both tables. An older such route could shadow a legitimate b
route on A. The current pre-dial logical scope check blocks the originally
reported unauthorized-backend example; that guard did not fix wrong candidates
or their availability consequences.

The schema Arc, compiled unit and backend selector remain shared across SNI
buckets. Accepted attachment indices borrow hostname lists from the schema;
there is no second owned hostname-list copy in the runtime unit. Rebuild locks,
affected-key updates, and per-listener immutable publication remain unchanged.
Internal records use `schemars(skip)` at the containing field and retain serde
transport; they are not operator-authored schema fields.

## Limits and verification

This does not establish cross-kind atomicity or general eligibility-aware route
selection. [Same-socket listener-specificity shadowing](../../../../tasks/todo/tlsroute-specific-listener-candidate-shadowing.md)
has a distinct single-attachment trigger and remains tracked. Existing
[protocol acceptance parity](../../../../tasks/todo/route-listener-protocol-acceptance-parity.md)
and [single-label hostname matching](../../../../tasks/todo/gateway-api-single-label-hostname-matching.md)
gaps also remain separate; this is not a full Gateway API conformance claim.
[Physical TLS listener manager recreation](../../../../tasks/todo/tls-listener-recreation-stale-route-manager.md)
is another pre-existing lifecycle defect: deleting every owning Gateway and
recreating it can disconnect configuration updates from the proxy's retained Arc.
Route-only deletion/recreation does not exercise that ownership transition.

Regression cases cover per-port hostname separation, accepted-parent filtering,
exact scoped ownership, wildcard narrowing, unrestricted listeners, invalid
empty input, schema visibility and transport, and preserved migration/old-snapshot
ownership assertions. The current run passed 64 Gateway TLS tests, the
Controller projection regression, resource hostname/transport regression, and
24 CRD/condition guards. Astra's deeper review found the rejected-route tracker
gap above; the correction passed its valid/invalid/recovery regression.
The subsequent P1 refresh correction adds real serial-worker regressions for
renames, same-socket optional-hostname exchanges, hostname exchanges across
existing ports, status/proof stability, delete/recreate, and dependency
registration before reads with dropped-parent count cleanup. The 105 Controller
Gateway-related tests, including all three processing-chain regressions, passed.
Astra's independent review approved the core correction and the authorized
integration-fixture migration. `cargo check --workspace --all-targets` and
`cargo clippy --workspace --all-targets` passed, with existing Clippy warnings.
The ClientHello integration regression passed after migrating its attachment
records and publishing the Gateway/GatewayClass scope fixtures. Its original
parser and backend-dial assertions remain intact. The `TlsRouteAttachments`
dynamic scenario is registered with its two physical ports. Historical results
on related pages are not fresh checks.

Live verification on 2026-09-13 used the canonical `prepare.sh`, then one shared
`start_all_with_conf.sh --suites TLSRoute/Basic,TLSRoute/Attachments` topology.
`test_client -g -r TLSRoute -i Basic` passed both cases;
`test_client -g --dynamic-scenario TlsRouteAttachments` passed all phases,
including Gateway listener rename, route-parent and hostname changes, wildcard
controls, exact backend identity, and Route delete/recreate. The final
`cache_residue --mode fail` passed after wiping 41 resources without errors.
Run the residue wipe after all traffic scenarios. An earlier attempt to reload
after that final wipe reproduced the separate manager-recreation defect above
and is not counted as a passing dynamic run.

Re-evaluate changes to attachment identity, parent acceptance, wildcard semantics,
runtime admission, and listener rebuild ownership.

## Resolution references

- `scope-tlsroute-effective-hostnames-to-attachments`: corrected projection;
  historical catch-all and unauthorized-backend premises narrowed as above.
