---
name: udp-session-receive-buffers
description: Assess idle UDP session memory and distinguish listener startup from receiver allocation.
knowledge_type: intentional-design
---

# UDP session receive buffers

For `gw-core3-05` item 2, retain one full-size receive buffer per admitted
session. Each listener starts with an empty session map, a semaphore count and
one 65,535-byte downstream receive buffer. In the [UDP proxy](../../../../edgion-gateway/src/routes/udp/edgion_udp.rs),
`get_or_create_session` reuses the current session for an existing direct peer.
A new session's `handle_upstream_packets` allocates its own reply buffer before
waiting for its first datagram. This keeps a complete datagram available to
the downstream send without contending for a shared buffer. With 10,000 live
sessions the requested reply-buffer capacity is about 625 MiB per listener.
See [session retirement](../regression-guards/udp-session-generation-retirement.md)
for cancellation and permit lifetime.

## Local idle-session baseline (2026-11-26)

Measured at `e8063a61a` on macOS 15.5 arm64, 16 GiB RAM, Rust 1.99.1,
with the Gateway binary's default jemalloc in an isolated Gateway library test
process. The deployment manifest requests 256 MiB and limits the Gateway to
1 GiB. Neither limit was imposed on this native process. Its file descriptor
limit was 20,000 and the ephemeral port range was 49152-65535.

In a temporary test copy, use the existing UDP unit helpers to create one
proxy on loopback. Call the production `get_or_create_session` once per
distinct peer with a fresh route slot, the fixture's pinned endpoint and a
common JoinSet. Keep the proxy and JoinSet alive. Each admitted session binds
its own upstream socket and starts its real receiver. With zero data packets,
check the session map size, JoinSet length and number of used permits agree at
each plateau. Sample process RSS with `ps -o rss=` three times, 700 ms apart.
Run the same binary with the receiver allocation switched from 65,535 bytes
to one byte as an idle-only attribution control. Use three fresh processes per
mode in order full/control/control/full/full/control.

| Buffer | Median process RSS in KiB at 0 / 1,000 / 10,000 sessions for each run | Median within-run RSS growth at 1,000 / 10,000 (KiB) |
|---|---|---|
| 65,535 bytes | 18592 / 23840 / 44992; 18432 / 23568 / 44848; 18432 / 23616 / 44848 | 5184 / 26416 |
| 1 byte | 18384 / 23168 / 40800; 18432 / 23168 / 40816; 18384 / 23104 / 40784 | 4736 / 22400 |

The differences in median growth were 448 KiB and 4,016 KiB. The requested
625 MiB at 10,000 sessions therefore did not translate to a comparable RSS
increase in this cold, zero-reply workload. This comparison measures user-space
RSS and excludes kernel socket memory. It does not cover a Linux container,
previously touched buffers, or burst traffic. A one-byte buffer is unsuitable
for real datagrams. Revisit pooling if deployments show material memory
pressure attributable to the full-size buffers.
