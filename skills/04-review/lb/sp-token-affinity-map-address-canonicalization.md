---
name: sp-token-affinity-map-address-canonicalization
description: Use when reviewing session-persistence stickiness findings claiming the SP token never resolves on IPv6 backends (bracket format mismatch) or misses the affinity map after a target-port rewrite; both are fixed by deriving the response token from the LB cell address (`lb_backend_addr`) via `std::net::SocketAddr::to_string()`, the same string the affinity map hashes.
status: fixed
finding_id: route-07 / route-08
closed: 2026-06-12
---

# Session-persistence token must hash the LB cell address, not the peer ip/port — FIXED

## Conclusion

The session-affinity token (`pg_upstream_response_filter.rs`) and the affinity-map key
(`lb::runtime_state::build_sp_affinity_map`) must hash the **same canonical address string** —
`std::net::SocketAddr::to_string()` of the **LB cell address** (`upstream.lb_backend_addr`,
i.e. `EdgionBackend.addr` before `resolve_target_port_key`). Two historical bugs broke this:

- **route-07 (IPv6 bracket mismatch):** the response side hashed `format!("{}:{}", ip, port)` where
  `ip` is `IpAddr::to_string()` → `::1:30001` (unbracketed), while the map hashed
  `SocketAddr::to_string()` → `[::1]:30001` (bracketed). Every IPv6-backend request missed the map.
- **route-08 (port-rewrite mismatch):** the response side hashed the **peer** address, whose port
  may have been rewritten by `resolve_target_port_key` (multi-port Services / stale pre-EndpointSlice
  sync), while the map is keyed by the un-rewritten cell port. The rewritten token permanently missed
  the map.

Both manifested identically: every request re-issued `InjectNew` and stickiness silently degraded to
plain LB. Both are fixed by one change — the response side now matches `upstream.lb_backend_addr`
(`SocketAddr::Inet(addr) => addr.to_string()`).

Re-reports of the "IPv6 token never sticks" or "token misses after port rewrite" forms are **not
accepted** — verify the response filter reads `lb_backend_addr` before re-reporting.

## Core Rationale

**1. One canonical address space + one canonical format.**

The affinity map is built from the service's real backend cells (`b.addr`, EndpointSlice address +
port, no rewrite) and keyed by `addr.to_string()`. Deriving the token from the same field via the
same `to_string()` makes the two strings byte-identical for both IPv4 and IPv6, eliminating both the
bracket-format and the port-space divergence in a single value-source change. The peer address was the wrong
source on both axes (format and port).

**2. `lb_backend_addr` is always set at injection time.**

Every state that triggers injection (`InjectNew` / `InjectUpdate`) ends with a peer selected by
`build_peer_from_lb_selection` (`backends/mod.rs`), which unconditionally sets
`upstream.lb_backend_addr = Some(lb_addr)`. When the sticky target resolves successfully the state
stays `Inactive` (no injection). So the new token source is never `None` on the injection path —
no regression to "no Set-Cookie".

**3. IPv4 behaviour is unchanged.**

For IPv4, `SocketAddr::to_string()` == `format!("{}:{}", ip, port)` == `a.b.c.d:port`, so existing
IPv4 tokens keep the same value (no client-visible cookie churn).

## Fix Suggestions Not Accepted

- "Bracket the IPv6 ip on the response side only (`format!("[{}]:{}", ip, port)` when v6)" — fixes
  route-07 but not route-08, and re-introduces a second ad-hoc formatter that can drift from the map.
- "Key the affinity map by the rewritten peer address instead" — moves the canonical space to the
  post-rewrite port, which varies per backend-ref and is not what the map is built from; harder to
  keep consistent than using the cell address on both sides.
- "Add HMAC / per-process secret to the token" — orthogonal; that is the separate
  `crc32-session-affinity-not-a-security-boundary` decision, not a fix for the resolve mismatch.

## Re-evaluation Triggers

Re-open only if:

- The affinity map build (`build_sp_affinity_map`) changes its key derivation away from
  `cell_addr.to_string()` — the response side must follow in lockstep.
- A new injection path is added whose peer is **not** selected by `build_peer_from_lb_selection`
  (so `lb_backend_addr` could be `None` at injection time).

## Reference Cases

- `tasks/todo/route-07-sp-ipv6-token-format-mismatch.md`, `route-08-sp-port-rewrite-token-miss.md`
  (sp-sticky-refresh design review, adversarial subagent, 2026-06-11), closed 2026-06-12.
- Same IPv6-authority-format family as `backends/ipv6-probe-url-brackets.md` (backends-F001),
  `backends/clusterip-ipv6-no-brackets.md` (backends-F017), `redirect/host-split-ipv6-literal.md`
  (redirect-F001).
- Regression test: `edgion-gateway/src/lb/runtime_state/mod.rs` →
  `tests::sp_affinity_round_trip_ipv4_and_ipv6`.
- Sibling SP decision: `skills/04-review/lb/crc32-session-affinity-not-a-security-boundary.md`.
