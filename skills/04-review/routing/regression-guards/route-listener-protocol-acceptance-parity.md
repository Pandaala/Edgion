---
name: route-listener-protocol-acceptance-parity
description: Use when reviewing Gateway Listener supportedKinds, Route parent acceptance, or attachedRoutes protocol compatibility.
knowledge_type: regression-guards
---

# Route attachment cannot override Listener protocol compatibility

## Conclusion and scope

For Gateway API Routes, a Listener accepts a Route kind only when the kind is
compatible with `Listener.protocol` and, when present, is selected by
`allowedRoutes.kinds`. The explicit kind list may narrow the protocol's Route
set; it cannot expand that set.

The Controller uses the same protocol-and-kind decision for Listener
`supportedKinds`, Route parent acceptance, TLSRoute attachment projection, and
the accepted-only `AttachedRouteTracker`. An incompatible explicit kind is
excluded from `supportedKinds`, sets the Listener's `ResolvedRefs` condition to
`False` with reason `InvalidRouteKinds`, rejects the Route parent with
`Accepted=False/NotAllowedByListeners`, and does not contribute to
`attachedRoutes`.

Non-Route attachments such as `EdgionTls` remain outside
`allowedRoutes.kinds`. A parentRef selecting multiple Listeners remains accepted
when at least one Listener is protocol-compatible and otherwise permits the
Route.

## Rationale and evidence

[Gateway API v1.5.0](https://github.com/kubernetes-sigs/gateway-api/blob/v1.5.0/apis/v1/gateway_types.go),
the version pinned by Edgion's current contract, states that each
`AllowedRoutes.kinds` entry must be compatible with the Listener protocol.
Unsupported entries must be omitted from
`ListenerStatus.supportedKinds` and reported through
`ResolvedRefs=False/InvalidRouteKinds`. Route attachment requires the Listener's
`AllowedRoutes` criteria to select the Route, so a protocol-incompatible entry
cannot create an attachment merely because its group and kind strings match.

`route_utils::protocol_supports_route_kind` owns the compatibility matrix.
`gateway::compute_supported_kinds` uses it to validate explicit kinds, while
`route_utils::listener_supports_route_kind` first applies it and then applies
the optional explicit allow-list. `validate_parent_ref_accepted` and
`update_tracker_accepted_only` therefore consume the same decision. TLSRoute's
projection retains its additional TLS-only guard as defense in depth.

Before the correction, `listener_supports_route_kind` returned true whenever an
explicit canonical group/kind matched. For example, an HTTP Listener that
listed TLSRoute could produce `TLSRoute Accepted=True` and increment
`attachedRoutes`, even though Gateway status rejected that kind and TLSRoute
projection discarded the non-TLS Listener.

## Alternatives and limits

Keeping separate protocol matrices in `gateway.rs` and `route_utils.rs` was
rejected because it allowed status and projection behavior to drift. Treating
an explicit kind as an operator override was rejected because it violates the
Gateway API compatibility rule and cannot make the data plane serve a different
protocol.

The Route rejection reason remains `NotAllowedByListeners`: no compatible
Listener's `AllowedRoutes` criteria permit the Route. `UnsupportedValue` is not
used for this case; the pinned reason documentation reserves it for an
unrecognized enum value. Listener `InvalidRouteKinds` and Route
`NotAllowedByListeners` describe different objects and must remain distinct.

This guard covers the canonical Gateway API group and Edgion's supported Route
kinds. Adding a custom Route group, a custom Listener protocol, or a new standard
Route kind requires extending the canonical matrix and its tests.

## Verification and re-evaluation

The Controller regression
`explicit_route_kind_cannot_override_listener_protocol` covers mixed compatible
and incompatible Listeners, the all-incompatible rejection reason,
`attachedRoutes` removal, and recovery after restoring a compatible protocol.
`supported_kinds_intersect_explicit_kinds_with_listener_protocol` verifies that
a mixed explicit list retains the valid kind and reports the invalid kind.

On 2026-09-19 both targeted tests passed:

- `cargo test -p edgion-controller explicit_route_kind_cannot_override_listener_protocol`
- `cargo test -p edgion-controller supported_kinds_intersect_explicit_kinds_with_listener_protocol`

Re-evaluate this rule when the supported protocol/kind matrix changes, custom
Route kinds are introduced, or attachment tracking stops consuming Route
acceptance results.

## Resolution references

- `route-listener-protocol-acceptance-parity` — fixed on 2026-09-19; source task
  created from the Astra review of
  `scope-tlsroute-effective-hostnames-to-attachments`.
