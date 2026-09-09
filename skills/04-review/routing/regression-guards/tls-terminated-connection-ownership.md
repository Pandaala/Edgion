---
name: tls-terminated-connection-ownership
description: Use when reviewing TLS termination route snapshot lifetime and handshake metadata copying.
knowledge_type: regression-guards
---

# TLS terminated connection ownership

## Conclusion and scope

`proxy.rs::select_route_unit` selects an owned `Arc<TlsRouteUnit>` synchronously.
The per-listener table guard ends before stream-plugin, connect, or relay awaits.
Existing connections retain their selected schema and backend selector generation;
new connections select from the current table.

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
The selected unit intentionally stays alive for connection configuration reads.

No temporary SNI take/restore, shared helper abstraction, new logging, or metrics
are introduced. Plugin decisions, denied/unsupported backend terminal gates,
weights, endpoint safety, retry/outlier tokens, timeout policy, PROXY v2 authority
and IPv6 handling, canonical duplex, and log output remain unchanged. Attachment
hostname projection, Stage-2 fallback removal, timeout pre-resolution, setup-write
deadlines, and the [TLS sensitive-log gap](../../../../tasks/todo/apply-sensitive-mode-to-tls-log-surfaces.md)
remain separate work; ownership optimization does not resolve those findings.

## Verification and re-evaluation

Inline tests in `edgion-gateway/src/routes/tls/proxy.rs` exercise production helpers.
Weak references cover two distinct published hostname winners: the old table and
unselected winner are released after rebuild, while the selected unit and selector
survive until the connection owner drops them. Tests also cover empty tables,
misses, case-insensitive matching, and next-generation selection.

Metadata tests cover Some/None, missing installed fields, mTLS, matched resource,
section and sync version, unchanged source metadata/client-certificate fields,
and the absence of handshake diagnostics in the connection context. They do not
perform a real TLS handshake, allocator instrumentation, or network integration.

Re-evaluate if selection stops returning an owned unit, another table guard or
metadata clone crosses connection awaits, or the set of installed TLS fields changes.

## Resolution references

- B0207: TLS termination snapshot retention and redundant metadata/address copies.
