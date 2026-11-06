# gw-core3-02: L4 stream policies only see the direct peer, so behind PROXY protocol every per-IP limit, IP list and GeoIP rule applies to the load balancer

- **Category**: design-error
- **Severity**: P2 (downgraded from P1 by independent verification; security and availability for L4 behind an L4 LB)
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/plugins/stream/stream_plugin_trait.rs:70-86` (`StreamContext` carries only `direct_peer_ip`)
  - `edgion-gateway/src/plugins/stream/connection_filter_bridge.rs:71-99` (Stage-1 listener filter, runs in `ConnectionFilter::should_accept` before the PROXY header is parsed)
  - `edgion-gateway/src/runtime/server/listener_builder.rs:525-536` (only a WARN when PROXY and `edgion-stream-plugins` coexist), `:573-597` (filter installed pre-handshake)
  - `edgion-gateway/src/routes/tcp/edgion_tcp.rs:185-196`, `:352-386` (`check_stream_plugins` builds `StreamContext` from `direct_peer_ip` although `TcpContext.remote_ip` already holds the PROXY source)
  - `edgion-gateway/src/routes/udp/edgion_udp.rs:330-335` (same helper)
  - `edgion-gateway/src/plugins/stream/connection_rate_limit/plugin.rs:97`, `:232` (`per_source_ip` key = `direct_peer_ip`)
  - Contrast: `edgion-gateway/src/plugins/stream/tls_route/ip_restriction.rs:29-33` (TLSRoute Stage 2 honours `IpSource::RemoteIp`)

## Summary
Inbound PROXY protocol exists precisely so an L4 load balancer (AWS NLB, GCP/Azure LB, HAProxy) can pass
the real client address. Edgion parses it and stores it as `remote_ip`, but the stream-plugin framework
never consumes it: Stage-1 listener plugins run before the header is read, and TCP/UDP route plugins are
handed only `direct_peer_ip`. With PROXY enabled, `ConnectionRateLimit.perSourceIp`, `IpRestriction`,
`GlobalConnectionIpRestriction` and `GeoIpLocation` all evaluate the LB address. Per-IP limits collapse
into one shared bucket (a single abusive client exhausts it for everyone) and allow/deny lists and geo
rules either block all traffic or none. Only TLSRoute Stage 2 offers `RemoteIp`.

## Current behavior (evidence)
- `StreamContext` has a single address field, `direct_peer_ip` (`stream_plugin_trait.rs:70-86`).
- Stage 1: `StreamPluginConnectionFilter::should_accept` receives Pingora's accept address and builds
  `StreamContext::new(canonical_peer_ip(addr), ...)` (`connection_filter_bridge.rs:71-99`). Pingora calls
  the connection filter in `ListenerEndpoint::accept()` *before* `proxy_protocol::apply()`
  (`listener_builder.rs:573-582` doc comment), so the PROXY source does not exist yet. The only mitigation
  is a startup WARN (`listener_builder.rs:525-536`).
- TCP route stage: `EdgionTcpProxy::handle_connection` has `ctx.remote_ip` (resolved from the PROXY
  header by `resolve_transport_client_addrs`) but calls `check_stream_plugins(..., &ctx.direct_peer_ip, ...)`
  (`edgion_tcp.rs:185-190`), which builds a `StreamContext` from that value only (`:379`). UDP reuses the
  same helper (`edgion_udp.rs:330-335`).
- `ConnectionRateLimit` keys its per-source limit as `ip:{port}:{ctx.direct_peer_ip}`
  (`connection_rate_limit/plugin.rs:232`).
- Validation rejects `source: RemoteIp` for L4 GeoIP with the rationale "no `X-Forwarded-For` to resolve"
  (`skills/02-features/03-resources/14-edgion-stream-plugins.md:141`), overlooking that PROXY protocol is the
  L4 equivalent and that the gateway already has the value.
- TLSRoute Stage 2 proves the model works: `TlsRouteIpRestriction` selects `IpSource::RemoteIp` vs
  `DirectPeerIp` (`tls_route/ip_restriction.rs:29-33`). TCPRoute/UDPRoute and all Stage-1 plugins lack it.

## Industry comparison
- Envoy: the `proxy_protocol` listener filter runs first and rewrites the downstream remote address;
  network filters (`rbac`, `local_ratelimit`, `connection_limit`) then match on `remote_ip` (PROXY source)
  or explicitly on `direct_remote_ip`, chosen per principal.
- HAProxy: `tcp-request connection expect-proxy layer4` followed by `tcp-request session` rules evaluates
  stick tables and ACLs on the PROXY-provided `src`.
- NGINX stream: `listen ... proxy_protocol` + `set_real_ip_from` makes `$remote_addr` the client for
  `limit_conn` / `allow` / `deny`.

## Impact
- AWS NLB with PROXY v2 (the standard way to preserve client IPs for TLS passthrough and TCP) makes every
  Edgion L4 security control ineffective or globally disruptive: an IP allow list must either include the
  LB (allowing everyone) or not (denying everyone); a per-IP connection rate limit throttles all clients
  together, turning one noisy client into a full L4 outage.
- Operators get a WARN at startup but no supported configuration that works; CLAUDE.md rule 12 explicitly
  expects RemoteIp to be usable behind a trusted proxy hop at L4.

## Suggested direction
1. Add `remote_ip` to `StreamContext` and pass `TcpContext.remote_ip` / UDP source into route-stage
   evaluation; give `IpRestriction`, `GeoIpLocation` and `ConnectionRateLimit` the same `ipSource`
   (`DirectPeerIp` default, `RemoteIp` opt-in) that TLSRoute Stage 2 already has, and allow `RemoteIp` only
   on listeners whose frozen PROXY policy is `TrustedSources`/`Required`.
2. For Stage 1 on PROXY listeners, either run the listener plugins after the PROXY handshake (a
   post-handshake hook in the Pingora fork, before TLS) or reject `RemoteIp`-dependent Stage-1 configs and
   steer users to the route stage — but stop silently evaluating the LB address.
3. Key `ConnectionRateLimit.perSourceIp` on the selected source, not unconditionally on the direct peer.

## Notes
- Checked `skills/04-review/plugin-runtime/`, `skills/04-review/ip-restriction/`, `skills/04-review/real-ip/`
  and `skills/02-features/03-resources/14-edgion-stream-plugins.md`: the L4 `RemoteIp` rejection is
  documented with an XFF-only rationale; no page evaluates PROXY-protocol deployments. Not covered by
  `gw-plugrt-05` (local per-IP limiting) or `gw-core-05` (session revalidation).

## Verification (independent)

Verdict: CONFIRMED, DOWNGRADED P1 -> P2.
- `StreamContext` carries only `direct_peer_ip` (`stream_plugin_trait.rs:68-86`); `check_stream_plugins` builds it from `ctx.direct_peer_ip` (`edgion_tcp.rs:185-190,352-386`) even though `TcpContext.remote_ip` holds the PROXY source.
- `ConnectionRateLimit` keys per-source on `direct_peer_ip` (`connection_rate_limit/plugin.rs:232`).
- `listener_builder.rs:525-536` emits only a startup WARN for PROXY + `edgion-stream-plugins`; `geo_rule.rs:279-287` rejects L4 `RemoteIp` because a stream resource may run before PROXY resolution.
- Downgrade reason: this is a known, WARN-surfaced limitation (missing capability) rather than a silently wrong security decision in default deployments; it only affects L4 listeners behind a PROXY-protocol LB that also use per-IP stream plugins. Route-stage (TCP/UDP) support is the cheap, high-value part of the fix.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `edgion-gateway/src/plugins/stream/stream_plugin_trait.rs` and `routes/tcp/edgion_tcp.rs` have no diff; `StreamContext` carries only `direct_peer_ip`.
- Only `connection_rate_limit/local_gate.rs` changed (`fetch_update` -> `try_update` rename); `plugin.rs:232` per-source key still `direct_peer_ip`.
