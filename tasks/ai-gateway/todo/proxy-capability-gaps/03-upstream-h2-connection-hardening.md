# Upstream H2 connection hardening — request cap and header-list bound

| Key | Value |
|-----|-------|
| Created | 2026-08-04 |
| Status | todo — cheapest of the three; the header bound is close to a one-liner |
| Where the change goes | Pingora fork (`pingora-core/src/connectors/http/v2.rs` and the H2 client options) |
| Effort | Low |
| Closes | gap 8 of `tasks/todo/transport-coverage-observability-gaps.md` (outbound Rapid Reset), correctly |

## Item A — `max_requests_per_connection` on the upstream H2 pool

**Default: 1000.**

**What it fixes.** A client that reliably trips a body policy makes us emit one HEADERS→RST_STREAM cycle per rejected request at the origin — the Rapid Reset shape, aimed **outward**. Our upstream H2 connections are shared across tenants, so an origin enforcing a post-CVE-2023-44487 reset budget can GOAWAY that connection **for everyone on it**. Capping requests per connection turns "one poisoned shared connection degrades every tenant on it" into a bounded, self-healing condition.

**Why a cap and not a reset rate limiter.** Rate-limiting our own resets is the wrong fix: it converts a cheap reset into a **held-open stream**, trading an outward-facing annoyance for an inward-facing resource cost. The cap does not have that inversion.

**Where the number comes from.** Envoy's `max_requests_per_connection` (`core.v3.HttpProtocolOptions` field 6) is documented to apply "for **both upstream and downstream** connections" — so the shape is right — but its default is **no limit**, so it cannot anchor the number. Take **1000** from nginx's `keepalive_requests`, which moved 100 → 1000 in **1.19.10** (1000 is current, not stale).

**State of the art, so nobody re-derives it.** No comparator defends the outbound direction with a budget:
- `h2`'s own reset accounting is gated on `initiator.is_library()` (`proto/streams/streams.rs:1648`) — **application-initiated resets are not counted at all**. Verified in-tree.
- nghttp2's reset limiter, the one thing that looked like an outbound defense, is documented as inbound and "**server use only**".
- Envoy *does* bound outbound RST_STREAM, but only incidentally: `max_outbound_control_frames` = 1000 is a **pending-queue depth cap that kills the connection**, not a budget or a rate.

The judgement recorded with this task: the absence is **architectural, not an oversight** — every design rejects before opening the upstream stream, which our own `body_gate` already does at `request_filter` — with a residue of "nobody has been bitten yet" for reset propagation on pooled connections. Item A is the belt to that braces.

## Item B — `max_header_list_size` on the upstream H2 client

**Default: 64 KiB. Consider a `max_headers_count` = 100 analog alongside it.**

We set `max_header_list_size` on the H2 **server** (`protocols/http/v2/server.rs:70`) and **nowhere on the upstream client**, so the upstream direction runs at `h2`'s default of **16 MiB**. Envoy's comparable bound is `max_response_headers_kb` = **60 KiB** (with `max_headers_count` = 100). 64 KiB is the round number in that neighbourhood.

A hostile or broken origin can otherwise make us allocate megabytes of header material per response.

## Verification bar

- **Item A** needs a test that the connection is actually retired at the cap — assert from the upstream side that a new connection is accepted at request N+1 (the seam harness records `ConnAccepted` per connection, so this is an exact count, not an inference). Assert the cap is honoured, not merely that requests still succeed.
- **Item B** needs a test that an over-large response header list is rejected rather than allocated. Confirm the rejection surfaces as an ordinary upstream error and does not poison the pooled connection for other streams.
- Both must show no change for ordinary traffic: a run below the cap opens exactly one connection, and normal-sized headers are unaffected.

## Not in scope

Per-source in-flight (concurrency) limits — `pingora-limits/src/inflight.rs` exists and is unwired, and nginx/Traefik both have the feature. It is deliberately deferred: it is a **policy** decision with tenant-visible side effects (nginx's `limit_conn` counts NAT'd clients against a shared `$binary_remote_addr` key, and under H2/H3 **each concurrent request counts as a connection**), and we already have rate limiting. Revisit if concurrency-shaped abuse actually appears.
