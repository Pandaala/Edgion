---
name: handshake-complete-fallback-no-double-log
description: handshake_complete_callback's no-snapshot fallback intentionally does not call log_ssl; the current callback lifecycle makes it unreachable after a successful BoringSSL flow, while first-store failures log and abort earlier. Reject re-reports claiming the fallback "silently drops" the connection.
knowledge_type: regression-guards
---

# tls-F004 — `handshake_complete_callback` fallback intentionally skips `log_ssl`

**Status:** ✅ fixed · 2026-05-27
**Severity:** Medium (BadLogic / observability)

## Conclusion

`handshake_complete_callback` in `edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs`
no longer calls `log_ssl(&meta)` in its no-snapshot fallback branch. It still
returns `Some(Arc::new(meta))` so downstream consumers (TLS proxy, HTTP proxy)
can read a minimal `TlsConnMeta` from the digest.

Re-reports of the form "the fallback drops the connection without an ssl.log
entry" or "no ssl.log when ex_data is unavailable" are **not accepted** —
this is deliberate to preserve the "one ssl.log per connection" audit
invariant.

## Pinned BoringSSL 5.2 reachability

`store_ssl_ctx` has two materially different call sites:

- The select-callback store is the first write for that SSL object. BoringSSL
  may need to create the ex-data stack and extend it through the registered
  index. Either allocation can fail; `store_ssl_ctx` then emits ssl.log and
  select_cb returns `SelectCertError::ERROR`, aborting before cert_cb or the
  completion callback.
- The finalize store reuses the same index after select_cb succeeded.
  `take_ssl_ctx` clears that existing slot by storing null; it does not shrink
  or destroy the stack. In BoringSSL 5.2, the later same-index store directly
  replaces the slot without allocating and therefore cannot return zero via
  the allocation-failure path.

The `finalize_ssl_ctx` failure branch remains defensive against a future
dependency or contract change. It is not a reachable heap-allocation path in
the pinned implementation and does not explain entry into the completion
fallback. The allocation performed by Rust's `Box::new(ctx.clone())` is also
not reported as an `SSL_set_ex_data` zero return.

## Why the fix is correct

Keeping the fallback non-logging preserves the established one-entry audit
policy without claiming that finalize-stage allocation failure is reachable:

- **BoringSSL builds.** A successful select_cb creates and sizes the ex-data
  stack, and the non-allocating finalize store restores the context for the
  completion callback. Reachable index-initialization or first-store failures
  emit ssl.log and abort the handshake before this callback. Reaching the
  fallback therefore requires a state-machine or ex-data contract violation,
  not ordinary allocation failure.
- **Non-BoringSSL builds.** `store_ssl_ctx` in `select_cb` always returns
  `false`, which short-circuits with `SelectCertError::ERROR`. Cert_cb and
  this callback are not reached on that path.

Because the fallback cannot determine whether an unknown future path already
emitted, it does not synthesize a second, partial audit entry. Any new reachable
path must carry an explicit emission state or otherwise preserve the one-entry
invariant.

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
- The pinned BoringSSL implementation changes so clearing or replacing an
  existing ex-data index can fail or discard the backing stack.

## Reference

- Issue: `tasks/tls/tls-F004-double-ssl-log-on-finalize-failure.md` (Medium · BadLogic)
- Code: `edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs` —
  `handshake_complete_callback` fallback branch + `store_ssl_ctx` failure
  emission in `ssl_ctx_ex_data`.
