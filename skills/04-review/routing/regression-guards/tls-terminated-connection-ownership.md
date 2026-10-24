---
name: tls-terminated-connection-ownership
description: Use when reviewing TLS termination route snapshot lifetime and handshake metadata copying.
knowledge_type: regression-guards
---

# TLS terminated connection ownership

## Conclusion and scope

TLSRoute Terminate is retained implementation, not an enabled product mode;
the Controller rejects explicit TLS/Terminate listeners. This page describes
that code without claiming production traffic or enabling the mode.

The shared `tls::select_tls_route` selects an owned `Arc<TlsRouteUnit>` synchronously.
The per-listener table guard ends before stream-plugin, connect, or relay awaits.
Connections retain their selected schema and backend selector generation through
admission and all connect attempts. After successful connection, they copy the
PROXY protocol version and drop the unit Arc before PP2 writes and relay.
New connections select from the current table.

`EdgionTlsTcpProxy::process_new` binds the owned Pingora SSL digest in a short
synchronous scope and borrows its `TlsConnMeta` extension. `install_tls_metadata`
copies only `tls_id`, `sni`, `matched`, and `is_mtls` into `TlsContext`. It does not
clone the full handshake record, including unused client-certificate, log, and
error data. Missing metadata leaves the context unchanged; absent metadata SNI
still permits the existing stream SNI fallback. The source extension is unchanged.

The endpoint address is borrowed from the existing backend Arc. The same
bracket-safe formatted String still feeds peer construction and PROXY v2.

## Rationale and evidence

Previously the asynchronous connection function retained the whole table even
though matching returned an owned unit. A long-lived connection could therefore
pin unrelated hostname winners after reload. Separately, the metadata path cloned
the entire handshake record before cloning three installed fields again.
These ownership reductions follow [Rust clone guidance](../../../03-coding/02-rust-coding-rules.md)
and the established [TCP](tcp-connection-snapshot-ownership.md) and
[TLS passthrough](tls-passthrough-connection-ownership.md) boundaries.

## Alternatives and limits

This does not promise immediate release of units held by other readers or the
canonical configuration cache, nor a measured latency or RSS improvement. The
SSL stream still owns its original digest throughout the connection; only the
unnecessary local metadata copy and digest handle are removed from the async scope.
The selected unit stays alive for connection configuration reads and retries.
After those reads, PP2 needs only the copied `Option<u8>`; context metadata and
the connected stream own their data independently. The compiled selector owns
slots, a WRR sequence and cursor, with no completion permit or drop callback.
The separate selected logical Gateway owner is unaffected.

No temporary SNI take/restore, shared helper abstraction, new logging, or metrics
are introduced. Plugin decisions, denied/unsupported backend terminal gates,
weights, endpoint safety, retry/outlier tokens, timeout policy, PROXY v2 authority
and IPv6 handling, canonical duplex, and log output remain unchanged. Attachment
hostname projection, Stage-2 fallback removal, timeout pre-resolution, setup-write
deadlines, and [TLS log address redaction](../../tls/regression-guards/tls-log-address-redaction.md)
are separate concerns; ownership optimization did not resolve those findings.

## Verification and re-evaluation

Tests in `edgion-gateway/tests/unit/routes/tls/proxy_test.rs` exercise metadata
and PP2 helpers. Weak references use the test-only `select_route_unit` ownership
view, not the production `select_tls_route` policy path. They cover two distinct
published hostname winners: the old table and
unselected winner are released after rebuild, while the selected unit and selector
survive until the connection owner drops them. Tests also cover empty tables,
misses, case-insensitive matching, and next-generation selection.

Metadata tests cover Some/None, missing installed fields, mTLS, matched resource,
section and sync version, unchanged source metadata/client-certificate fields,
and the absence of handshake diagnostics in the connection context. They do not
perform a real TLS handshake, allocator instrumentation, or network integration.

Re-evaluate if selection stops returning an owned unit, another table guard or
metadata clone crosses connection awaits, the set of installed TLS fields changes,
or later setup/relay needs route-owned data or gains unit-owned lifecycle effects.
The explicit post-connect drop is supported by source ownership and compilation;
existing tests do not exercise its production location. No allocation-count,
async-frame-size or throughput improvement is measured.

## Resolution references

- B0207: TLS termination snapshot retention and redundant metadata/address copies.
