# gw-plugin2-03: RegionRoute forwards across regions with no hop/loop guard and trusts request-carried routing hints on every hop

- **Category**: design-error
- **Severity**: P2 (worth doing)
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/plugins/http/region_route/plugin.rs:241-300` (`route_to`: only `local_region == region.name` stops forwarding)
  - `edgion-gateway/src/plugins/http/region_route/plugin.rs:302-372` (`exec_hash_range`: `hash_key_get` pass-through at `:315-326`, write-back at `:357-361`)
  - `edgion-gateway/src/plugins/http/region_route/plugin.rs:393-410` (`RouteByDirectRegion` from `key_from`)
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/region_route.rs:20-160` (config: no hop/loop field)

## Summary
RegionRoute makes a request jump to another region's gateway (`UpstreamJumpTarget::External`) when
the selected region is not the local one. The receiving gateway runs the same plugin again and
recomputes the decision. Nothing marks a request as already region-forwarded, and nothing limits
hops. When two regions disagree about which region owns a key, the request ping-pongs between
regions until the client or upstream timeout fires. A disagreement is normal during a rollout of a
new `ClusterRegionSetting` hash range, `matchMap` or `failoverTo`, because each region's config
arrives independently. The only cross-hop coordination is a request-carried value: the
`hashKeyGet` pass-through, or a `RouteByDirectRegion` header. That value is also readable from
client input, so clients can pick their own region.

## Current behavior (evidence)
- `route_to` (`:241-300`): if `local_region == region.name`, the plugin passes. Otherwise it sets an
  external jump to `region.backend_endpoint`, with SNI taken from the client `Host` and
  `preserve_raw_path: true`. There is no header added, checked or incremented (no `Via`, no
  `x-edgion-region-hop`, no visited-region list).
- `exec_hash_range` (`:315-326`): if `hashKeyGet` yields a value `< modulo`, it is used verbatim
  (`hash_pass`). `write_back` (`:357-361`) is the mechanism meant to keep the next hop consistent,
  but it carries the *hash*, not the *region decision*. The next hop maps that hash with its own
  current region table. When region tables differ between clusters, A maps the hash to B and B maps
  it to A, giving an unbounded loop.
- `apply_failover_redirect` (`:205-239`) is a static per-region rewrite. A↔B failover declarations
  that do not match across clusters produce the same loop.
- The config struct has no hop limit or "already routed" marker, so operators cannot add one.

## Industry comparison
- **HTTP itself (RFC 9110 §7.6.3)**: intermediaries append `Via` and use it to detect forwarding
  loops. NGINX/Envoy deployments commonly reject a request carrying their own `Via` or node token.
- **Envoy**: `x-envoy-…` internal headers are stripped at the edge unless trusted
  (`use_remote_address`, `internal_address_config`). Multi-cluster routing (Istio east-west gateways)
  marks cross-cluster traffic and never re-applies locality routing to a request that arrived
  through the east-west gateway.
- **Cloudflare / Akamai multi-region steering**: a single decision is made at the first hop and
  pinned. Downstream PoPs honor it from a trusted, signed or internal-only header and do not
  recompute.

## Impact
- During config rollouts or region-table edits, affected keys bounce between regions. Each hop
  holds a downstream connection and an upstream connection in both regions until timeout. This is
  cross-region bandwidth plus a connection-exhaustion amplifier, and it surfaces as client
  timeouts that are hard to diagnose.
- When the routing hint source is a client-readable header (the natural choice for
  `hashKeyGet`/`key_from`), clients can choose a region. This matters when regions are used for data
  residency or tenant isolation.

## Suggested direction
1. Add a hop marker. On jump, set a gateway-owned header (for example `x-edgion-region-from:
   <local>` plus a hop counter). On ingress, strip it unless the direct peer is a trusted
   region-peer (reuse `DirectPeerIp` + `IpGroup` trust). If it is present and trusted, **do not
   recompute**. Either accept the request locally or, if a hop limit (default 1) is exceeded, fail
   with a fixed 508/502 and an `EdgionErrorCode`.
2. Pin the decision rather than the hash. Forward the chosen region name in the trusted marker, so
   that a stale table at the next hop cannot redirect again.
3. Document that `hashKeyGet` / `RouteByDirectRegion` sources must be gateway-written or
   peer-trusted values. Or restrict them to a trusted-peer-only accessor when the plugin is used as
   a placement control.

## Notes
- Checked `skills/04-review/routing/regression-guards/region-route-effective-table-validation.md`.
  It validates hash-range coverage within one table, not cross-region consistency or loops. Also
  checked `skills/04-review/ssrf/mandatory-design/upstream-address-safety.md`, which covers
  dial-time address safety only.
- No related task under `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED (severity P2 unchanged)
- `edgion-gateway/src/plugins/http/region_route/plugin.rs:241-292`: `route_to` stops only on `local_region == region.name`; otherwise it sets an External jump with no marker header, hop counter or visited list.
- No `hop`/`via`/`loop` handling exists in `region_route/plugin.rs` or `edgion-resources/.../plugin_configs/region_route.rs`; the config has no field to add one.
- `skills/04-review/routing/regression-guards/region-route-effective-table-validation.md` covers single-table coverage only.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `plugins/http/region_route/plugin.rs` has no diff; `route_to` (`:245-250`) still stops only on `local_region == region.name`.
- `region_route.rs` config unchanged; `docs/.../region-route.md` diff adds a dye YAML example only.
