---
name: sni-passthrough-clienthello-read
description: Use when reviewing the TLS passthrough SNI extraction path; explains the bounded read loop with absolute deadline that mirrors Envoy's tls_inspector, why parsing never proceeds against a truncated record, and how trailing 0-RTT EarlyData is forwarded verbatim.
knowledge_type: regression-guards
---

# SNI Passthrough: ClientHello read contract

## Conclusion

The TLS passthrough proxy reads the downstream ClientHello via a bounded read
loop with an absolute deadline, parses the SNI from a guaranteed-complete
record, and forwards **every byte drained from downstream** (not just the
ClientHello record) to upstream — preserving any 0-RTT EarlyData the client
coalesced into the same TCP segment.

Suggestions of the form "fall back to parsing whatever bytes arrived on a
short read" are **not accepted** — that was the original bug (RFC review
finding `tls-03`). Suggestions of the form "only forward the ClientHello
record to upstream" are **not accepted** — that silently drops 0-RTT
EarlyData and was caught in the second review pass for the same finding.

## Design vs Envoy

| Property | Edgion (`passthrough_proxy.rs`) | Envoy `tls_inspector` |
|----------|---------------------------------|-----------------------|
| Max bytes drained for inspection | `MAX_CLIENT_HELLO_LEN = 16384` (RFC 8446 §5.1 `2^14`) | `max_client_hello_size` aligned with the same TLS record ceiling |
| Read budget | `SNI_READ_DEADLINE = 15 s` (absolute) | `listener_filters_timeout` default 15 s |
| Buffer model | Consume + replay: drained bytes are buffered and forwarded to upstream verbatim | `MSG_PEEK`: bytes stay in the socket buffer and the upstream connection re-reads them |
| Action on timeout | Close connection + `err_log` | Close connection (default) |
| Action on cap exceeded | Close connection + `err_log` | Close connection + counter |
| First-byte != 0x16 | Close connection + `err_log` | Close connection |

Edgion uses consume-and-replay (rather than `MSG_PEEK`) because Pingora's
`Stream` abstraction does not expose `MSG_PEEK`. The two models are
behavior-equivalent for the bytes inside the inspection buffer; anything
beyond `MAX_CLIENT_HELLO_LEN` is naturally relayed by the bidirectional
`duplex()` once the inspection phase completes.

## Code shape

`sni_parser.rs` exposes two functions:

- `client_hello_record_complete(buf) -> RecordCompleteness` — cheap probe.
  Returns one of `Complete { record_end }`, `NeedMore`, or `NotTlsHandshake`.
- `extract_sni(buf) -> Option<String>` — runs only after the caller has
  observed `Complete`. If the buffer is internally truncated it returns
  `None` defensively rather than clamping (defense in depth).

`passthrough_proxy.rs::read_client_hello` is the read loop. It returns
`(record_end, filled)`:

- `record_end` — offset just past the ClientHello record; `buf[..record_end]`
  is fed to `extract_sni`.
- `filled` — total bytes drained from downstream (`filled >= record_end`).
  The caller forwards `buf[..filled]` to upstream, so any bytes the client
  coalesced after the ClientHello (notably TLS 1.3 0-RTT EarlyData records)
  reach the backend verbatim.

Timeout is enforced via `tokio::time::timeout_at(Instant::now() + SNI_READ_DEADLINE, ...)`.
The absolute (not per-read relative) deadline defeats slowloris-style dribble:
an attacker feeding one byte at a time cannot push the cumulative wait past
the bound.

## Why not …

- **MSG_PEEK** — Pingora's `Stream` does not expose it. Consume-and-replay
  delivers the same end-to-end behavior.
- **Per-read relative timeout** — would reset on each byte; dribble bypass.
- **Forward only the ClientHello record** — drops trailing 0-RTT EarlyData
  that the client may have packed into the same TCP segment.
- **Unbounded buffer** — `MAX_CLIENT_HELLO_LEN = 16384` matches the TLS
  record fragment ceiling (`2^14`, RFC 8446 §5.1).

## Re-reports to reject

| Pattern | Why we already cover it |
|---------|-------------------------|
| "Short read should parse what arrived" | That was the bug; reproduces `tls-03` |
| "Need a read timeout" | Already present (`SNI_READ_DEADLINE = 15 s`, absolute deadline) |
| "Need an upper bound on the inspection buffer" | Already present (`MAX_CLIENT_HELLO_LEN = 16384`) |
| "Need to reject non-Handshake first bytes early" | `client_hello_record_complete` returns `NotTlsHandshake` on `buf[0] != 0x16` before any further read |
| "Trailing 0-RTT EarlyData is silently dropped" | Already fixed: `read_client_hello` returns `(record_end, filled)`; the caller forwards `buf[..filled]` so trailing EarlyData is preserved |
| "extract_sni should also tolerate truncated records" | No — extract_sni's contract is "complete record"; the read loop guarantees that. Tolerating truncated input here would re-introduce the original bug |

## Reference

- Source: `edgion-gateway/src/routes/tls/sni_parser.rs`, `edgion-gateway/src/routes/tls/passthrough_proxy.rs::read_client_hello`
- Envoy reference: `source/extensions/filters/listener/tls_inspector/tls_inspector.{cc,h}` and `api/envoy/config/listener/v3/listener.proto` (`listener_filters_timeout`)
- RFC: RFC 8446 §5.1 (record layer), RFC 6066 §3 (SNI extension)

## Resolution references

- `tls-03` — 2026-05-19.
