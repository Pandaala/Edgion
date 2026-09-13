---
name: routes-http-listener-manager-per-request-lookup
description: Use when reviewing findings that `pg_request_filter` / `EdgionHttpProxy` calls `get_or_create_listener_manager(&listener_key)` per request and incurs a DashMap shard write-lock plus a `ListenerKey(String)` clone on the data-plane hot path; this is a real fix — manager Arcs are cached on the proxy at construction.
knowledge_type: regression-guards
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# `EdgionHttpProxy` Caches Per-Listener Route Manager `Arc`s (REAL FIX)

## Conclusion

`pg_request_filter` used to call
`get_global_http_route_managers().get_or_create_listener_manager(&edgion_http.listener_key)` —
and the gRPC-flavored equivalent — on every HTTP / gRPC request. That lookup is
**not** a cheap read:

1. `key.clone()` allocates a fresh `String` (`ListenerKey(pub String)`) per call.
2. `DashMap::entry(...)` acquires a **shard write guard**, even on the hit path,
   because the `entry` API may insert.
3. All requests on one listener share the same `ListenerKey` and therefore hash
   to the same shard — concurrent requests on a single listener serialize on
   that one write lock on the hottest path.

The looked-up `Arc<HttpPortRouteManager>` / `Arc<GrpcPortRouteManager>` retains
identity across Route updates while its Gateway listener remains configured:

- `EdgionHttpProxy` is constructed once per listener in
  `listener_builder.rs` and shared as `&self` by Pingora across all requests on
  that listener.
- Route updates only swap the inner `ArcSwap<DomainRouteRules>`; the outer
  `Arc` returned by `get_or_create_listener_manager` retains identity.
- `rebuild_*_listener_managers` only sweeps `by_listener` entries that are both
  route-less **and** absent from `ListenerGatewayInfoStore`. That store tracks
  logical Gateway ownership, not live sockets. Deleting every owner can remove
  the manager while a proxy still retains its cleared `Arc`; recreation then
  allocates a different manager. The earlier claim that every live proxy stays
  registered was incorrect. See the accepted
  [Gateway recreation boundary](../../listeners/intentional-design/gateway-recreation-requires-restart.md).

**Fix**: add `http_route_manager: Arc<HttpPortRouteManager>` and
`grpc_route_manager: Arc<GrpcPortRouteManager>` fields to `EdgionHttpProxy`,
populate them once at construction via
`get_or_create_listener_manager(&listener_key)`, and have `request_filter`
read them directly. The per-request route table read still goes through the
inner `ArcSwap` (`.load_route_table()`), preserving Route hot reload under an
existing Gateway. It does not establish Gateway delete/recreate recovery.
This eliminates the per-request `String` allocation and the shard
write-lock entirely on the request path.

## Why the Naive Form Was Wrong

- Treating `entry(key.clone()).or_insert_with(...)` as a "lookup" — it is a
  write-locking insert on every call. `read()` / `get(&key)` would be a read
  lock, but the call site needed the write semantics for first-time creation,
  which is exactly what makes it unsuitable for the hot path.
- Assuming `ListenerKey::clone()` is cheap — it owns a `String`, so it heap
  allocates.
- Assuming the global manager map is the right place to look up a value
  whose binding is fixed for the caller's lifetime — when the value is stable
  for the caller, cache it on the caller.

## Pattern to Apply Elsewhere

For any per-request `DashMap`/global-map lookup where:

1. The key is derivable at construction of a longer-lived struct that holds
   `&self` across requests, AND
2. The lookup uses `entry` / write-acquiring access, AND
3. The returned value is identity-stable for the caller's lifetime
   (mutation goes through interior `ArcSwap` / `Mutex` / `RwLock`),

…cache the resolved value on the caller. Keep the global map for lifecycle
management (insert on listener up, sweep on listener down); do not consult it
on the request path.

## Reference

- F-001 (`tasks/routes/routes-http-F001-per-request-dashmap-listener-lookup.md`),
  closed 2026-05-28.
- Source:
  `edgion-gateway/src/routes/http/proxy_http/mod.rs` (struct fields),
  `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs` (read sites),
  `edgion-gateway/src/runtime/server/listener_builder.rs` (construction).
- Stability rationale in `routes_mgr.rs`:
  `rebuild_all_listener_managers` / `rebuild_affected_listener_managers`
  preserve outer `Arc` identity while the key remains registered; stale sweep
  uses logical `active_listeners` (= `ListenerGatewayInfoStore::all_listener_keys`),
  not serving-proxy liveness. This rationale was corrected by source inspection
  on 2026-09-13; the historical performance fix was not rerun for that correction.
