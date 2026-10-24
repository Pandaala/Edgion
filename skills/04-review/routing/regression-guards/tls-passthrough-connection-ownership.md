---
name: tls-passthrough-connection-ownership
description: Use when reviewing TLS passthrough route snapshot lifetime and consume-and-replay buffer ownership.
knowledge_type: regression-guards
---

# TLS passthrough connection ownership

## Conclusion and scope

The shared `tls::select_tls_route` is a synchronous selection boundary:
only the selected `Arc<TlsRouteUnit>` survives into asynchronous plugin,
connect and retry work. The per-listener table guard does not pin unrelated
hostname winners after a configuration swap. The selected schema and backend
selector remain the connection's original generation through all attempts.
After successful connection, the proxy copies the PROXY protocol version and
drops its unit Arc before PP2 writes, ClientHello replay and relay.

`process_new` truncates its inspection Vec to `filled` and moves it into
`handle_connection`. After any configured PROXY v2 header, the owned
`replay_initial_bytes` helper writes and flushes all consumed bytes, increments
the existing upstream counter, then explicitly drops the Vec before relay.
Truncation does not shrink or copy the allocation. The 16 KiB inspection heap
allocation is no longer owned by the caller throughout the connection.

The endpoint address is borrowed from the retained backend Arc instead of
cloning the address value. Its formatted bracket-safe String still serves
both peer construction and the PROXY v2 destination.

## Rationale and evidence

The former async function retained a named table guard despite receiving an
owned unit Arc from matching. Separately, its caller borrowed an inspection
buffer into that same long-running function. Neither owner was needed during
relay. The correction follows [Rust clone guidance](../../../03-coding/02-rust-coding-rules.md)
and the established [TCP ownership boundary](tcp-connection-snapshot-ownership.md).

## Alternatives and limits

Replay preserves the [ClientHello read contract](../../tls/regression-guards/sni-passthrough-clienthello-read.md):
SNI parsing uses `record_end`, but forwarding uses every `filled` byte, including
trailing early data. Write or flush failure is terminal, does not enter relay,
and does not increment the initial-byte counter, matching prior behavior.
No replay retry, new timeout policy, or replacement relay is introduced.

This is an ownership guarantee, not a measured RSS reduction, allocator return
to the OS, or async future-frame size claim. Other readers and the canonical
configuration cache can still retain units. TLS termination, attachment-scoped
hostname projection, Stage-2 fallback removal, sensitive-log modes, timeout
pre-resolution, and setup-write deadline investigation are outside this change.
PROXY v2 ordering/address handling, endpoint safety, retry/outlier behavior,
plugin decisions, and existing connection logs are unchanged.

PP2 needs only the copied `Option<u8>` after selection and retries finish.
Context metadata and the connected stream own their data independently; the
compiled selector owns ordinary slots, a WRR sequence and cursor without a
completion permit or drop callback. The separate logical Gateway owner remains
unchanged. Releasing this local Arc is not proof of immediate deallocation.

## Verification and re-evaluation

Tests in `edgion-gateway/tests/unit/routes/tls/passthrough_proxy_test.rs` exercise
the replay helper. Weak handles use the test-only `select_route_unit` ownership
view, not the production `select_tls_route` policy path, and prove old-table and unselected hostname-winner
release after a swap, selected unit/selector survival, new-generation selection,
and misses. The two old units own distinct hostnames, so this is not a test of
collision-loser release. In-memory short writes verify record-plus-tail bytes
arrive once; partial-write and flush failures preserve diagnostics and counters.
These tests do not instrument allocation deallocation or execute a real TLS
handshake, listener configuration sync, or network integration scenario.

Verified with `cargo test --locked -q -p edgion-gateway --lib routes::tls`
(52 passed, including four passthrough tests) and the same command with
`routes::l4_duplex` (21 passed). Targeted rustfmt, `git diff --check`, and
agent-doc validation passed.

Re-evaluate if a connection loads another table guard across I/O, selection
stops returning an owned unit, inspection/replay/relay ownership changes, or
later setup needs route-owned data or gains unit-owned lifecycle effects.
Source ownership and compilation establish the explicit post-connect drop;
existing tests do not drive that production location. No allocation-count or
throughput improvement is measured.

## Resolution references

- B0206: TLS passthrough table and inspection-buffer retention reduction.
