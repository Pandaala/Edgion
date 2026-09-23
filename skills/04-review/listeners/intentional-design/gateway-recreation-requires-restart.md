---
name: gateway-recreation-requires-restart
description: Gateway deletion and recreation require a data-plane restart; distinguish retained-manager recovery from supported Route and certificate hot reload.
knowledge_type: intentional-design
---

# Gateway recreation requires a data-plane restart

## Conclusion and scope

Gateway dynamic deletion/recreation is outside the supported lifecycle. After
recreating Gateway resources, restart or roll the data-plane processes against
the desired configuration, even when the listener address and protocol are
unchanged. Route and certificate hot reload under an existing Gateway remains
supported; Controller dependency requeue remains required.

On 2026-09-13 the user explicitly clarified this product boundary and approved
closing the listener-manager recovery task without a production fix. The
observed recovery failure is accepted within that unsupported workflow, not
fixed or disproved. This decision does not declare every Gateway field immutable
or disable existing configuration synchronization.

## Rationale and evidence

The [startup contract](../../../01-architecture/02-gateway/01-startup-shutdown.md)
separates listener construction from runtime route publication. Current source
inspection confirms the following ownership chain:

- `runtime/server/listener_builder.rs` gives each HTTP/gRPC, TCP, TLS terminated,
  TLS passthrough, or UDP proxy its per-listener manager `Arc`.
- `runtime/store/listener_gateway_info.rs::all_listener_keys` inventories logical
  Gateway owners, not physical proxy liveness. Gateway updates rebuild it through
  `runtime/handler.rs`.
- HTTP/gRPC `routes_mgr.rs` and shared `routes/l4_rebuild.rs` clear and remove a
  `by_listener` entry when both logical owners and route references disappear.
  An existing proxy still owns the cleared manager.
- Recreating the same `ListenerKey` allocates a distinct manager. Restored routes
  update that new manager, while the old proxy continues loading its empty
  `route_table`. Waiting for additional synchronization cannot reconnect them.

Historical HTTP evidence recorded 404/`RouteNotFound` after a successful TLS
handshake; terminated TLS evidence recorded no selected TLSRoute after Gateway
reapply. These are route reachability observations, not certificate-matching
failures. The later [retained-manager wire guard](../regression-guards/retained-listener-manager-clearing-wire-coverage.md)
verified HTTP, gRPC, and TCP on macOS; TLS and UDP remain outside that wire
scenario.

## Alternatives and accepted cost

The accepted cost is unavailable routing after deleting all logical owners and
recreating them on an existing process. Restart is the supported recovery path.
Keeping construction-time handles avoids a write-acquiring global lookup on
every request/connection. Empty publication and reclamation of removed route
contents remain required; retaining stale routes is not an availability fix.

The same boundary applies to the Gateway-level backend TLS identity fallback.
`GatewayHandler::full_set` updates `GatewayBackendTlsStore` for Gateway keys in
the incoming snapshot but does not prune identities for omitted Gateway keys.
Removing a client certificate or its Secret while the Gateway remains present
still publishes that Gateway and removes or replaces the identity normally.
Only omitting the Gateway key from a later authoritative snapshot can leave the
identity store-owned until the required data-plane restart. Resource deletion,
GatewayClass or namespace filtering, and runtime ownership reassignment are
examples of that unsupported logical removal. The Admin API's
`gateway_backend_tls_store.count` can consequently exceed the current Gateway
snapshot size until restart. Removing that store owner would not erase
outstanding `Arc<Secret>` readers or cryptographic copies, and it does not make
dynamic Gateway removal/recreation supported. The proposed authoritative
replacement was therefore declined as unnecessary work for the unsupported
lifecycle, not because the residual owner or count divergence was disproved.

A serving-owned stable handle or recoverable registration could implement
same-socket recovery, but requires an explicitly scoped lifecycle design across
protocols, publication, and bounded reclamation. The user declined that scope.
This is a product choice, distinct from the
[Pingora runtime bind/drain constraint](../compatibility-constraints/listener-port-set-frozen-at-startup.md):
the dependency limitation alone does not prove same-socket recovery impossible.

The [retained-handle clearing coverage](../regression-guards/retained-listener-manager-clearing-wire-coverage.md)
checks fail-closed routing; it does not promise later recovery.
Established connections, pinned snapshots, and UDP sessions retain their existing
lifetime contracts; this decision does not require rematching them.

## Verification and re-evaluation

Current HTTP/gRPC retained-handle unit tests and TCP helper tests assert empty
publication and distinct identities on recreation. They were inspected, not
rerun for this documentation-only decision. Historical integration failures are
preserved in the related routing knowledge; no failed recovery test is relabeled
as passing.

The later targeted `RetainedListenerHandles` dynamic scenario passed on macOS
on 2026-09-23. Linux CI execution was not verified; the user chose to close
the coverage task without that result. This newer evidence does not change the
restart boundary or retroactively make the 2026-09-13 checks a wire test.

The two `BothAbsentParentRef` recovery tests now exercise Controller dependency
requeue by renaming a Listener without changing its physical key. They verify
the refreshed Controller/Gateway projections and preserve live traffic without
claiming support for Gateway deletion and recreation.

Documentation verification on 2026-09-13 passed scoped local-link and whitespace
checks, `cargo fmt --all`, `cargo check --workspace --all-targets`,
`cargo clippy --workspace --all-targets` (with warnings), agent-doc validation,
SSA force, metrics inventory, and Pingora source policy. Initial format/build/doc
attempts failed during concurrent, unrelated hostname/hash_host/radix_tree source
relocation; the required retries passed after that work advanced. Independent
read-only review returned LGTM for the accepted disposition, source evidence,
knowledge corrections, and retained test follow-up. No full or targeted runtime
tests were run for this documentation-only decision.

Re-evaluate if Gateway dynamic lifecycle support is explicitly authorized, if
the serving framework gains runtime listener management, or if the same failure
occurs during Route-only/certificate-only updates while a Gateway remains
configured. The last case is outside this acceptance and needs investigation.

## Resolution references

- `http-listener-recreation-stale-route-manager` and
  `tls-listener-recreation-stale-route-manager`: accepted unsupported-lifecycle
  recovery limitation on 2026-09-13; no production fix.
- `replace-gateway-backend-tls-identities-authoritatively`: accepted residual
  Gateway identity-store ownership in the same unsupported deletion lifecycle;
  supported in-place certificate removal and replacement remain unchanged.
- Source task: `listener-route-manager-lifecycle`.
