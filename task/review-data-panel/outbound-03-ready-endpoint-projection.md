# Shared outbound ready-endpoint projection

Status: deferred representation decision for user review. No runtime change.
Source baseline: 74fbe609c.

## Source evidence

EpSliceStore::ready_endpoints_for_port in
`edgion-gateway/src/backends/discovery/endpoint_slice/ep_slice_store.rs` validates
a nonzero u16 Service port, captures one discovery generation and resolves that
Service section. It copies the section's entire ready_slots address sequence into
an owned Vec<SocketAddr>. collect::<Option<Vec<_>>>() returns an empty list if
any address is not Inet. This is published readiness, not live health/outlier
selection. Numeric Service ports identify sections; returned sockets retain the
EndpointSlice serving ports, which can differ from the Service port.

`plugins/http/common/outbound/target.rs::ResolvedTarget::pick_url` obtains that
vector, advances its local cursor for a nonempty list, selects one socket and
formats a URL. The vector is then discarded. Static targets already borrow their
URL and are outside this candidate. ManagedOutboundClient::dial_url still applies
its literal-dial validation to the selected URL.

`plugins/http/common/outbound/pinned_resolver.rs::AnchorPinnedResolver` is the
other production consumer. It filters unsafe addresses, advances its separate
cursor only for a nonempty filtered list, rotates the list and transfers it into
reqwest's owned address iterator. The requested hostname is deliberately ignored;
there is no DNS fallback. Resolver rotation distributes new connections, not every
request on already pooled connections.

## Decision alternatives

- Retain current copying if endpoint counts and exchange frequency make it cheap.
- Introduce a narrow synchronous borrowed selection operation for the scalar URL
  consumer, keeping discovery ownership local and returning only a SocketAddr.
  Preserve whole-list validation; choosing one slot alone must not silently erase
  rejection of an invalid sibling. Avoid duplicating section resolution logic.
- Prepare a numeric ready-address projection within the existing immutable section.
  This trades repeated copying/validation for additional persistent storage and
  publication cost. Do not add a second registry or a stale request-local cache.
- Consider an owning iterator for the resolver separately. Keeping a section or
  generation alive through connector consumption can retain much more memory than
  the current compact socket Vec. Rotation after safety filtering must remain exact;
  laziness must not change which addresses are offered or the captured generation.

No design or net performance benefit is selected. Measure allocation requests,
selection cost, retained heap, reload overlap and connector consumption lifetime
for small/large Services and varying new-connection rates. Distinguish the scalar
call path from pooled resolver invocations. Compare peak memory as well as average
throughput; eliminating one Vec is insufficient evidence by itself.

## Acceptance for later implementation

Capture one current generation per invocation, preserve section resolution and
invalid/missing Service/port behavior, exact ready order and multiplicity, serving
ports and whole-list non-Inet failure. Keep the current cursor fetch-add/modulo
semantics, including empty-list non-advancement and wrap behavior. Pinned safety
filtering precedes rotation; scalar selection retains its existing later dial
validation. Do not add health/outlier policy or permit fallback DNS resolution.

Cover heterogeneous serving ports, empty/unavailable sections, invalid ports,
publication replacement, all-filtered resolver results, queried-name independence,
rotation and IPv4/IPv6. Include deterministic allocation/ownership comparisons and
both scalar and resolver consumers. Preserve static target behavior, authority
policy, TLS identity and final egress checks. Any implementation requires a frozen
scope, independent IMPL plan/final review and project checks.

This differs from [selected-slot lookup](backend-01-selected-slot-lookup.md),
which maps an already selected LbBackend back to its proof, and from
[URL serialization storage](outbound-02-url-serialization-storage.md).
Original knowledge: skills/01-architecture/02-gateway/07-backends.md and
skills/04-review/backends/regression-guards/named-port-resolve-against-originating-slice.md.

Fresh baseline passed 123 shared outbound, 41 discovery coordinator and four direct
EndpointSlice facade inventory tests (168 total), formatting and workspace
all-target check/Clippy. Existing unrelated warnings remain. No allocation or
throughput benchmark ran; tests do not prove a proposed representation's benefit.
