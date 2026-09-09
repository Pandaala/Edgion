---
name: tcp-connection-snapshot-ownership
description: Use when reviewing TCP connection route selection lifetime and context-owned upstream address construction.
knowledge_type: regression-guards
---

# TCP connection snapshot ownership

## Conclusion and scope

`EdgionTcpProxy::handle_connection` obtains its owned `Arc<TcpRouteUnit>` through
the private synchronous `select_route_unit` helper. The table guard is dropped
before stream-plugin evaluation, upstream connect, or relay can suspend. An
existing connection keeps its selected schema and compiled runtime generation;
new connections select from the latest table.

The upstream address String is moved into `TcpContext.upstream_addr` before
address validation and borrowed for `BasicPeer` construction. Unsafe-address
failures retain the same diagnostic address and bounded denial reason. No
additional copy of the formatted address is needed for peer construction.

## Rationale and evidence

Previously a named route-table guard lived in the async connection function
until it returned. Although `TcpRouteTable::match_route` already returned an
owned unit Arc, a long-lived connection also retained the entire old table and
all its other candidates after a reload. The synchronous selection boundary
limits this retention without copying schema or changing first-match ordering.
This follows [Rust clone guidance](../../../03-coding/02-rust-coding-rules.md)
and complements [configuration-time schema sharing](tcp-route-schema-arc-sharing.md).

## Alternatives and limits

The selected unit intentionally remains alive; retaining no route generation
would invalidate the connection's borrowed configuration. This change does not
promise immediate release when another connection or configuration cache still
owns a unit. It does not alter global cache ownership, listener publication,
stream-plugin live lookup, endpoint safety, outlier completion tokens, or
timeout resolution. No TLS/UDP lifetime change or measured latency claim is
implied.

Relay behavior remains owned by the [shared L4 relay](l4-duplex-relay-centralization.md).
TCP maps downstream flush errors to `DownstreamWriteError`; counters measure
bytes read, including bytes awaiting a blocked write, not confirmed delivery.
Clean EOF preserves success; idle closure reports `IdleTimeout`.

## Verification and re-evaluation

Inline tests in `edgion-gateway/src/routes/tcp/edgion_tcp.rs` exercise the actual
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
another table guard across I/O, or relay hook/error semantics change.

## Resolution references

- B0199: TCP connection snapshot retention and redundant address-copy removal.
