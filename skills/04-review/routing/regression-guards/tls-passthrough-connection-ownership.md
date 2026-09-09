---
name: tls-passthrough-connection-ownership
description: Use when reviewing TLS passthrough route snapshot lifetime and consume-and-replay buffer ownership.
knowledge_type: regression-guards
---

# TLS passthrough connection ownership

## Conclusion and scope

`passthrough_proxy.rs::select_route_unit` is a synchronous selection boundary:
only the selected `Arc<TlsRouteUnit>` survives into asynchronous plugin,
connect, and relay work. The per-listener table guard does not pin unrelated
hostname winners after a configuration swap. The selected schema and backend
selector intentionally remain the connection's original generation.

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

## Verification and re-evaluation

Inline tests in `edgion-gateway/src/routes/tls/passthrough_proxy.rs` exercise the
production helpers. Weak handles prove old-table and unselected hostname-winner
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
stops returning an owned unit, or inspection/replay/relay ownership changes.

## Resolution references

- B0206: TLS passthrough table and inspection-buffer retention reduction.
