---
name: udp-established-session-route-pipeline-per-packet
description: Use when reviewing findings that flag EdgionUdpProxy::handle_client_packet for running the full route pipeline (ArcSwap load, route match, backend select, round-robin, SSRF) on every UDP packet of an established session; this was a real defect, now fixed with a session fast-path.
knowledge_type: regression-guards
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# UDP `handle_client_packet` Per-Packet Route Pipeline for Established Sessions — FIXED (F-016)

## Conclusion

**Confirmed-real, now fixed (F-016, 2026-05-26).** `EdgionUdpProxy::handle_client_packet`
(`edgion-gateway/src/routes/udp/edgion_udp.rs`) used to run the full route pipeline —
`udp_route_manager.load_route_table()` (ArcSwap load), `route_table.match_route()`,
`rule.backend_finder.select()`, `select_roundrobin_backend_key()`, address resolution, and
`validate_endpoint_backend_addr()` (SSRF) — unconditionally on **every** UDP packet, only
calling `get_or_create_session` afterward. For an already-established session that entire
pipeline was pure redundant work on the data-plane hot path.

The fix adds a fast-path guard at the top of `handle_client_packet`: if
`self.client_sessions.get(&client_addr)` hits, the packet is sent straight to the session's
stored `upstream_addr` over its dedicated `upstream_socket` and the function returns before
the pipeline runs.

Verify the current `handle_client_packet` carries the
`client_sessions.get(..).map(|s| s.value().clone())` fast-path before concluding that
route match, backend selection, or SSRF checks repeat for established sessions.
The historical correction does not establish that every per-packet cost or session
lifecycle behavior is correct.

## Core Rationale

**1. The pipeline result was already discarded for established sessions**

`get_or_create_session(client_addr, upstream_addr)` returns the cached session on a hit and
**ignores** the freshly computed `upstream_addr`; the actual send always targets
`session.upstream_addr`, which is pinned at session-creation time. So skipping the pipeline
for existing sessions does not change the destination — it only stops computing a value that
was thrown away.

**2. Hot path, scales with packet rate × session count**

UDP packet handling is the data-plane hot path. Under high-rate long-lived sessions
(DNS / VoIP / gaming / IoT telemetry) the redundant ArcSwap load + route match + backend
select + round-robin + SSRF validation ran on every packet across up to
`MAX_UDP_SESSIONS` (10000) concurrent sessions.

**3. Lock-safety in the fix**

The session `Arc` is cloned out via `.get(..).map(|s| s.value().clone())` so the DashMap
read guard is dropped before the `send_to(..).await`, avoiding holding a shard lock across an
`.await` point.

## Intended behavior change (documented, accepted)

- Established sessions now have **session affinity**: route/backend config changes apply only
  to sessions created after the change. Existing sessions keep forwarding to their original
  upstream until they expire after `SESSION_TIMEOUT` (60 s of inactivity), at which point the
  next packet re-runs route matching. This is the standard UDP-proxy model and is documented
  in `docs/en/user-guide/udp-route/overview.md` (Notes §3).
- An established session no longer drops packets if the route table later changes to
  `NoRouteMatched` / `NoBackendSelected`; it stays pinned until expiry. This keeps long-lived
  sessions alive across transient config churn.

## Re-evaluation Triggers

Re-evaluate if:

- A requirement appears to apply route/backend changes to **in-flight** UDP sessions
  immediately (would need explicit session invalidation on route update, not a per-packet
  re-match).
- The session fast path changes, or synchronization fields gain consumers whose
  lifetime is independent of the owning session.

## Session-owned bookkeeping and deferred address formatting

`ClientSession` is already shared through `Arc<ClientSession>` by the session map,
packet handlers, and cleanup. Its activity mutex and four atomic counters have no
independent owners, so they live inline in the session rather than behind five
additional Arcs. This removes five allocations per constructed session; the mutex,
relaxed counter ordering, and update/read sites are unchanged. It is a structural
allocation reduction, not a measured throughput or RSS claim.

Successful established-session sends do not need a textual client IP. The upstream
send-result helper therefore takes `SocketAddr` and calls the shared
`canonicalize_ip` only on failure, matching the downstream helper. New-session
route/plugin processing still obtains the canonical IP before policy evaluation.
Failure entries retain the same status, peer port, canonical IPv4-mapped spelling,
and `UdpLogEntry` sensitive-data serialization. No new log surface is introduced.

The socket, permit, cancellation/task ownership, pinned destination, bind-family
selection, address-safety gate, packet buffer, and session limit remain unchanged.
The 60-second timeout is inactivity-based, not a maximum lifetime for active
sessions. Retirement races, upstream sender validation, and shutdown draining are
separate residual issues; this optimization does not correct or accept them.

Verification uses the existing inline UDP tests and UDP log serialization/status
tests. Existing socket-based tests are preserved, not expanded. No allocator
instrumentation or new integration fixtures are introduced; these checks do not
establish live-session retirement or cross-process hot-reload correctness.

## Reference Cases

- F-016 (BadLogic, Medium), fixed 2026-05-26.
- Source: `edgion-gateway/src/routes/udp/edgion_udp.rs` `handle_client_packet` fast-path.
- Doc: `docs/en/user-guide/udp-route/overview.md` (Notes §3).
- B0212: inline session bookkeeping and error-only upstream peer formatting.
