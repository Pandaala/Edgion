---
name: l4-duplex-relay-centralization
description: Use when reviewing TCP / TLS-terminated / TLS-passthrough proxy changes that touch the bidirectional `select!` relay loop. The skeleton lives in one place — `routes/l4_duplex.rs` — and is parameterized via the `DuplexHooks` trait; do not re-introduce per-route duplicates.
knowledge_type: regression-guards
---

# L4 Duplex Relay — Centralized in `l4_duplex.rs`

**Audit date**: 2026-05-26
**Linked issue**: `path-norm-nonhttp-routes` F-010 (closed by extracting the shared helper).

## Conclusion

All L4 (TCP / TLS-terminated / TLS-passthrough) bidirectional relay traffic now
flows through a single helper:

- `pub(crate) async fn bidirectional_relay(downstream, upstream, hooks)` in
  `edgion-gateway/src/routes/l4_duplex.rs`
- Trait `DuplexHooks { record_sent, record_received, on_error }` plugs in the
  per-context bits.
- Error variants: `DuplexError::{UpstreamWrite, DownstreamRead, DownstreamWrite,
  DownstreamFlush, UpstreamRead}`.

Concrete hook impls:

- `TcpDuplexHooks` — in `edgion-gateway/src/routes/tcp/edgion_tcp.rs`. Maps errors
  to `TcpStatus::*`. Note: `DownstreamFlush` collapses to
  `TcpStatus::DownstreamWriteError` — this matches pre-refactor TCP behavior
  (which never distinguished flush from write failure).
- `TlsDuplexHooks` — in `edgion-gateway/src/routes/tls/mod.rs`, shared by both
  `proxy.rs` (terminate) and `passthrough_proxy.rs`. Writes free-form strings
  into `ctx.err_log`. `DownstreamFlush` produces `"downstream flush err"`.

## Don't re-report

Do not file new findings of the form "TCP and TLS proxies duplicate the same
`tokio::select!` relay loop / 8 KiB buffer constants / `Ok(0)` break pattern" —
the duplication was deliberately removed. Each `duplex(...)` method on the
three proxies is now a one-line dispatch to `bidirectional_relay`. If a fourth
L4 proxy variant appears, route it through the same helper instead of cloning
the loop body.

## When changing the relay

Changes that need to land in **one** place now (instead of three): buffer size,
read/write ordering, flush policy on the client-facing side, EOF handling, new
error categories, timeouts. Cross-check:

- `l4_duplex.rs` — the skeleton.
- `TcpDuplexHooks::on_error` — mapping to `TcpStatus`.
- `TlsDuplexHooks::on_error` — mapping to `err_log` strings.
- If you add a new `DuplexError` variant, every hook impl must cover it
  (compiler-enforced via the exhaustive `match`).
- Idle timeout (landed 2026-06-02, finding fr-l4-02): `bidirectional_relay` takes
  `idle_timeout: Option<Duration>` and is bounded by a single idle clock reset on
  any byte progress (read or partial write) via `write_all_idle` / `flush_idle`.
  `DuplexError::IdleTimeout` maps to `TcpStatus::IdleTimeout` and the TLS
  `"idle timeout"` err_log. Value comes from `resolve_tcp_idle_timeout(spec.tcpTimeout)`.

## Behavior contract preserved by the helper

- Two independently-owned 8 KiB heap buffers.
- `Ok(0)` (clean EOF) on either side breaks without invoking `on_error`.
- Byte counters are recorded *before* the matching write, so they reflect bytes
  read from the source even if the subsequent write fails.
- Client-facing direction is `flush`-ed after every upstream → client write;
  upstream direction is not flushed (matches prior behavior across all three
  call sites).
