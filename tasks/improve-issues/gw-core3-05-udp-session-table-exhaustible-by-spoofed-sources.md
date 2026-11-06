# gw-core3-05: The UDP session table is exhaustible by spoofed sources, and each half-open session pins a socket, a task and a 64 KiB buffer for 60 s

- **Category**: design-error
- **Severity**: P2 (worth doing; availability of UDPRoute listeners)
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/routes/udp/edgion_udp.rs:37` (`SESSION_TIMEOUT` 60 s, same for replied and unreplied sessions), `:44` (`MAX_UDP_SESSIONS` 10 000 per listener, fixed), `:436-506` (`get_or_create_session`: permit + dedicated `UdpSocket::bind` + spawned receiver per new source address), `:510-562` (`handle_upstream_packets`: per-session `vec![0u8; 65535]` and a 1 s `recv_from` timeout loop), `:569-609` (only sends refresh `last_activity`), `:637-660` (`cleanup_expired_sessions` every 10 s)
  - `edgion-gateway/src/routes/udp/edgion_udp.rs:330-335` (the only per-source control is the optional route-stage stream plugin, keyed on the spoofable source)
  - `edgion-gateway/src/runtime/server/listener_builder.rs:360-397` (UDP listeners get no Stage-1 connection filter)

## Summary
Every new UDP source address that passes route selection immediately consumes one of 10 000 per-listener
session permits, binds a dedicated ephemeral upstream socket, spawns a receiver task with its own 64 KiB
receive buffer, and forwards the datagram upstream. The session lives for 60 s whether or not the backend
ever answered. UDP sources are trivially spoofable, so roughly 170 forged packets per second (10 000 per
minute) keep the table permanently full: every legitimate new client then gets `SessionLimitReached` and
is dropped, while the gateway holds 10 000 sockets, 10 000 tasks (each waking once per second) and up to
~640 MiB of receive buffers per UDP listener. The backend also receives traffic "from" the forged sources.

## Current behavior (evidence)
- Admission: `acquire_session_permit` on a `Semaphore::new(MAX_UDP_SESSIONS)` (`:44`, `:436-456`). No
  per-source-IP or per-subnet quota exists; the limit is a non-configurable constant.
- Resource cost per session: one `UdpSocket::bind` (one fd), one tracked task, and
  `let mut buf = vec![0u8; MAX_UDP_PACKET_SIZE]` (`:519`) held for the session lifetime; the receiver loop
  wakes every second through `tokio::time::timeout(Duration::from_secs(1), ...)` (`:529`) even when idle.
- Lifetime: `last_activity` is refreshed on any successful send in either direction (`:579`, `:607`), and
  the creating packet itself counts. A session whose backend never replies (the normal outcome for a
  forged source, or a backend that drops unknown clients) still lives the full `SESSION_TIMEOUT`.
- No pre-admission defence: UDP listeners have no Stage-1 connection filter (`listener_builder.rs`
  `add_udp_listener` installs none), and route-stage `ConnectionRateLimit`/`IpRestriction` evaluate the
  forged source address itself.

## Industry comparison
- Linux conntrack (the model most UDP proxies imitate) uses a short timeout for unreplied UDP flows
  (`nf_conntrack_udp_timeout` 30 s) and promotes only flows that saw a reply (`ASSURED`,
  `udp_timeout_stream` 120–180 s); unreplied entries are also the first evicted under pressure.
- Envoy `udp_proxy`: shared per-worker sockets with `recvmmsg` batching and pooled buffers
  (`upstream_socket_config`, `prefer_gro`), session limits through cluster circuit breakers
  (`max_connections`), and `use_per_packet_load_balancing` to avoid per-source session state entirely for
  stateless protocols such as DNS.
- NGINX stream UDP: `proxy_responses` lets a session end as soon as the expected replies arrive (e.g. 1
  for DNS), so sessions do not linger for a fixed idle window.

## Impact
- A low-rate spoofed flood denies service to all new clients of every UDPRoute on the listener
  (DNS, syslog, game, QUIC-adjacent services), and the gateway cannot distinguish attackers because the
  source is forged.
- Memory and fd pressure scales with listeners × 10 000 regardless of real traffic; the 1 s polling adds
  ~10 000 timer wakeups per second per saturated listener.

## Suggested direction
1. Two-tier lifetime: a short unreplied timeout (for example 5–10 s, or until the first upstream reply)
   and the existing idle timeout only after the backend has replied; under permit pressure, evict the
   oldest unreplied sessions first.
2. Add a per-source (and optionally per-/24 or /64) session quota in front of the global permit.
3. Make the session cap and timeouts EGC-configurable alongside `tcpTimeout`, and add a `proxy_responses`
   style "expected replies" setting so request/response protocols release sessions immediately.
4. Lower the per-session footprint: drop the 1 s timeout loop (rely on cancellation/retirement), size the
   receive buffer from the listener MTU or use a shared per-listener buffer pool.

## Notes
- Checked `skills/04-review/listeners/regression-guards/udp-session-creation-failure-cause.md`,
  `udp-session-generation-retirement.md`, `udp-upstream-response-source.md`,
  `skills/04-review/cpu-memory/regression-guards/udp-established-session-route-pipeline-per-packet.md` and
  the comment on `SESSION_TIMEOUT` (a `udpTimeout` knob was deferred "until a concrete need appears"). None
  evaluates spoofed-source admission or unreplied-session lifetime; this finding supplies that need.
- Distinct from `gw-core-05` (session revalidation and single serve task head-of-line latency) and
  `gw-plugrt-05` (Redis-only per-IP connection limiting on TCP accept).

## Verification (independent)

Verdict: CONFIRMED. Severity P2 kept.
- `SESSION_TIMEOUT` 60 s and fixed `MAX_UDP_SESSIONS` 10 000 per listener (`edgion_udp.rs:30-44`).
- Each session allocates `vec![0u8; 65535]` and polls with a 1 s `timeout` loop (`edgion_udp.rs:519-529`); `last_activity` is refreshed on upstream sends too (`:575-580`), so an unreplied spoofed session lives the full window.
- No per-source quota or unreplied/replied distinction exists; the deferred `udpTimeout` knob comment does not consider spoofed admission. Distinct from `gw-core-05`.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `edgion-gateway/src/routes/udp/edgion_udp.rs` has no diff: fixed `MAX_UDP_SESSIONS`, 60 s `SESSION_TIMEOUT`, per-session 64 KiB buffer and socket remain.
- `listener_builder.rs` unchanged; UDP listeners still have no Stage-1 filter.
