---
name: handshake-complete-fallback-no-double-log
description: handshake_complete_callback's no-snapshot fallback intentionally does not call log_ssl; emission belongs to the upstream store_ssl_ctx failure path. Reject re-reports claiming the fallback "silently drops" the connection.
---

# tls-F004 — `handshake_complete_callback` fallback intentionally skips `log_ssl`

**Status:** ✅ fixed · 2026-05-27
**Severity:** Medium (BadLogic / observability)

## Conclusion

`handshake_complete_callback` in `src/core/gateway/tls/runtime/gateway/tls_pingora.rs`
no longer calls `log_ssl(&meta)` in its no-snapshot fallback branch. It still
returns `Some(Arc::new(meta))` so downstream consumers (TLS proxy, HTTP proxy)
can read a minimal `TlsConnMeta` from the digest.

Re-reports of the form "the fallback drops the connection without an ssl.log
entry" or "no ssl.log when ex_data is unavailable" are **not accepted** —
this is deliberate to preserve the "one ssl.log per connection" audit
invariant.

## What was wrong (before the fix)

When `SSL_set_ex_data` returned 0 inside `store_ssl_ctx` invoked from
`finalize_ssl_ctx` (BoringSSL heap allocation failure), the sequence was:

1. `store_ssl_ctx` emitted ssl.log with a full meta (tls_id / sni / matched
   stamped) and `"exdata store failed"`. **First entry.**
2. `finalize_ssl_ctx` returned without further emission; cert was already
   applied to the SSL object so the handshake proceeded.
3. `handshake_complete_callback` ran, `take_ssl_ctx` returned `None`
   (ex_data slot was zeroed by the earlier `take_ssl_ctx` in cert_cb and
   never re-stored), and the fallback emitted a second ssl.log with
   `"no ssl ctx"` — missing tls_id / sni / matched. **Second entry.**

Two entries for one connection violated the audit invariant; the second
entry's missing fields could mislead anomaly detection.

## Why the fix is correct

Removing the `log_ssl` call from the fallback is safe because every realistic
path that lands here has already emitted ssl.log upstream with a more
complete meta:

- **BoringSSL builds.** The only ways `take_ssl_ctx` returns `None` after a
  successful select_cb are (a) `store_ssl_ctx` failed inside `finalize_ssl_ctx`
  (it emitted ssl.log with the stamped `tls_id`), or (b) `select_cb` failed
  and was somehow still followed by a handshake (currently impossible — a
  `SelectCertError::ERROR` return aborts the connection before cert_cb).
- **Non-BoringSSL builds.** `store_ssl_ctx` in `select_cb` always returns
  `false`, which short-circuits with `SelectCertError::ERROR`. Cert_cb and
  this callback are not reached on that path.

The remaining theoretical case — BoringSSL state-machine glitch zeroing
ex_data between callbacks — is unreachable-by-design. Losing an `ssl.log`
entry there is the accepted trade-off for not double-counting the rare
allocation-failure case.

The digest meta is still returned, so `TlsProxy` / HTTP proxy / access log
consumers continue to receive a minimal `TlsConnMeta` (port + sni at best).

## Re-evaluation triggers

- A new upstream path is added that can land in `handshake_complete_callback`
  without any prior emission — then a flag must be threaded through (or this
  fallback must re-emit) to keep the audit invariant.
- The non-BoringSSL `store_ssl_ctx` stops short-circuiting in `select_cb` (for
  example, if rustls support gains real ex_data handling) — then the fallback
  becomes a legitimate first-emission point and must log again.
- The `take_ssl_ctx` semantics change so that successful finalize no longer
  guarantees recovery (e.g., split callbacks / async transitions).

## Reference

- Issue: `tasks/tls/tls-F004-double-ssl-log-on-finalize-failure.md` (Medium · BadLogic)
- Code: `src/core/gateway/tls/runtime/gateway/tls_pingora.rs` —
  `handshake_complete_callback` fallback branch + `store_ssl_ctx` failure
  emission in `ssl_ctx_ex_data`.
