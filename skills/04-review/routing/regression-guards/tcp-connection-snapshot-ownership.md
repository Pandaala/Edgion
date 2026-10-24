---
name: tcp-connection-snapshot-ownership
description: Use when reviewing TCP connection route selection lifetime and context-owned upstream address construction.
knowledge_type: regression-guards
---

# TCP connection snapshot ownership

## Conclusion and scope

`EdgionTcpProxy::handle_connection` obtains its owned `Arc<TcpRouteUnit>` through
the private synchronous `select_route_unit` helper. The table guard is dropped
before stream-plugin evaluation, upstream connect, or relay can suspend. A
connection keeps its selected schema and compiled runtime generation through
admission, backend selection and peer preparation. After copying keepalive into
the peer, it explicitly drops its unit Arc before upstream connect and relay.
New connections select from the latest table.

The upstream address String is moved into `TcpContext.upstream_addr` before
address validation and borrowed for `BasicPeer` construction. Unsafe-address
failures retain the same diagnostic address and bounded denial reason. No
additional copy of the formatted address is needed for peer construction.
The source socket address is borrowed from the retained backend for formatting
and synchronous safety validation, rather than cloning its enum. The borrow
ends before connection I/O; backend ownership for outlier completion is unchanged.
Internet socket-address cloning was a value copy, so this is not a heap-allocation
or measured throughput claim.

## Rationale and evidence

Previously a named route-table guard lived in the async connection function
until it returned. Although `TcpRouteTable::match_route` already returned an
owned unit Arc, a long-lived connection also retained the entire old table and
all its other candidates after a reload. The synchronous selection boundary
limits this retention without copying schema or changing first-match ordering.
This follows [Rust clone guidance](../../../03-coding/02-rust-coding-rules.md)
and complements [configuration-time schema sharing](tcp-route-schema-arc-sharing.md).

## Alternatives and limits

The selected unit must remain alive while admission and preparation borrow its
configuration. The earlier whole-connection retention description was too broad:
the selected backend owns its endpoint independently, keepalive copies durations
and counts, and the later connect/relay path has no route-unit borrow. Its compiled
selector owns slots and WRR storage, with no completion permit or custom drop
callback. Releasing this connection's Arc does not promise immediate deallocation
when another connection or configuration cache still owns a unit. It does not
alter global cache ownership, listener publication,
stream-plugin live lookup, endpoint safety, outlier completion tokens, or
timeout resolution. No TLS/UDP lifetime change or measured latency claim is
implied.

Relay behavior remains owned by the [shared L4 relay](l4-duplex-relay-centralization.md).
TCP maps downstream flush errors to `DownstreamWriteError`; counters measure
bytes read, including bytes awaiting a blocked write, not confirmed delivery.
Clean EOF preserves success; idle closure reports `IdleTimeout`.

## Verification and re-evaluation

Tests in `edgion-gateway/tests/unit/routes/tcp/edgion_tcp_test.rs` exercise the actual
selection helper with Weak references: old-table and unselected-unit release,
selected-unit survival, and new-generation selection after a swap. Ordinary
in-memory duplex streams exercise `TcpDuplexHooks` through the production relay
for a clean bidirectional exchange and paused-clock idle closure while an
upstream write is blocked. Direct callbacks cover every error-to-status mapping
and cumulative counters. These tests use no network or global configuration
mutation, and do not establish a fresh transport integration result.

Verification filters: `routes::tcp` (22 passed), `routes::l4_duplex` (21 passed),
and `observe::logs::tcp_log` (4 passed), using
`cargo test --locked -q -p edgion-gateway --lib`. Targeted rustfmt check,
`git diff --check`, and agent-doc validation also passed.

Re-evaluate when selection stops returning an owned unit, connection code loads
another table guard across I/O, prepared peer/backend data borrows the unit, a
unit-owned resource gains lifecycle effects, or relay hook/error semantics change.

The explicit pre-connect unit drop is established by the production ownership
boundary and compiler checks. The existing Weak-reference selection test checks
generation independence and release after its own final owner is dropped; it
does not drive the production connect path or measure reload-time memory. No
allocation-count, RSS, async-frame-size or throughput improvement is claimed.

## Resolution references

- B0199: TCP connection snapshot retention and redundant address-copy removal.
