---
name: ssl-ctx-exdata-handoff-audit-ownership
description: Use when reviewing store_ssl_ctx clone cost, owned SslCtx handoff proposals, or finalize-store dual ssl.log reports; current borrow-and-clone store is accepted and the secret-copy claim is stale.
knowledge_type: intentional-design
---

# SslCtx ex-data store stays borrow-and-clone; finalize dual-emit is accepted while unreachable

## Conclusion and scope

Keep `store_ssl_ctx(&mut SslRef, &SslCtx)` in
`edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs`. Do not treat the
per-store `ctx.clone()` as a correctness defect, and do not start an owned
handoff plus setter-failure audit refactor unless a re-evaluation trigger
below fires.

This decision covers the BoringSSL Gateway handshake only: select →
certificate → completion / `free_ssl_ctx`. It does not change certificate
matching, FIPS policy, mTLS validation, the rustls-unsupported boundary, or
the tls-F004 rule that `handshake_complete_callback`'s no-snapshot fallback
must not call `log_ssl`.

The claimed "clone duplicates Secret vectors and certificate/key bytes" is
false for current code. `SslCtx` holds `Option<Arc<CompiledEdgionTls>>` and
`Option<Arc<GatewayTlsEntry>>`; clone bumps those Arcs and copies
`TlsConnMeta` (SNI, `tls_id`, bounded `LogBuffer`, match labels). Compiled
serving certificates stay behind
`compiled_serving_certificates: Option<Arc<[Arc<CompiledServingCertificate>]>>`.
That secret-copy claim was true when `SslCtx.matched_gateway_tls` was an
owned `GatewayTlsEntry` with `secrets: Option<Vec<Secret>>` (`82fcfda3`);
`8cbbed6c` replaced it with the Arc plus precompiled model.

A separate contract hole remains: if finalize-stage `SSL_set_ex_data`
returns 0, `store_ssl_ctx` emits `ssl.log` (`exdata store failed`) and
`certificate_callback` emits again after `finalize_ssl_ctx` sets
`err_log = "finalize: store ctx failed"`. `log_ssl` only appends. This is
an accepted unreachable risk on pinned BoringSSL 5.2, not correct
one-entry behavior and not a production-reachable bug. Select-callback
store failure already emits once and returns `SelectCertError::ERROR`.

## Rationale and evidence

Canonical one-entry audit text remains
[downstream TLS](../../../01-architecture/02-gateway/04-tls/01-downstream-tls.md)
and [tls-F004](../regression-guards/handshake-complete-fallback-no-double-log.md).
Design rule U does not make this clone a MUST: it is two bounded metadata
copies plus `Box<SslCtx>` per successful handshake, not per-request regex
or engine rebuild.

`CRYPTO_set_ex_data` in boring-sys 5.2
(`crypto/ex_data.cc`) returns 0 only when creating or growing the ex-data
stack fails. After a successful select store, `take_ssl_ctx` writes null
into the same index without shrinking the stack; finalize restore is an
in-place replace and returns 1. Rust `Box::new` failure is not reported as
`SSL_set_ex_data == 0`. First-store failure in select remains the only
ordinary allocation-failure path and already has a single audit owner.

`take_ssl_ctx` already refuses `Box::from_raw` when clearing the slot
fails, leaving ownership with BoringSSL. That memory-safety check is
independent of this acceptance.

On 2026-09-19 the user declined the owned-handoff / exactly-once-audit
todo after current-code verification: the secret-copy premise is stale,
and the finalize dual-emit is a real contract gap that pinned BoringSSL
does not open.

## Alternatives and limits

Benefit of keeping the current API: no handshake-callback ownership
rewrite, no setter-failure injection seam, no change to the four-callback
ordering.

Accepted cost:

- Two cheap `SslCtx` clones per successful handshake.
- If finalize `SSL_set_ex_data` ever returned 0, one connection would get
  two terminal `ssl.log` records (store then caller). Completion fallback
  still does not emit a third (tls-F004). The certificate is already
  installed, so the handshake may still complete.

Rejected for this close:

- Consume `SslCtx` in `store_ssl_ctx` and return it on failure.
- Unify finalize failure logging now without that rewrite.
- Add a setter-failure injection matrix solely to prove the unreachable
  branch.

Do not close a report that current `SslCtx` holds owned secrets or that
clone copies PEM/private-key bytes: that premise is disproved. Do not
close a report that finalize dual-emit is reachable on a new TLS stack
without re-checking `CRYPTO_set_ex_data`.

## Verification and re-evaluation

Verified by reading current `tls_pingora.rs`, `GatewayTlsEntry`,
`TlsConnMeta` / `LogBuffer`, git history (`82fcfda3` → `8cbbed6c`), and
boring-sys 5.2 `CRYPTO_set_ex_data`. No fault-injection test was added
and no production dual-emit was observed. Independent review and the
repository quick-check set for this closure are recorded in the closing
run, not as a new handshake audit.

Reconsider when any of the following is true:

- `SslCtx` again stores `GatewayTlsEntry` or Secret material by value.
- Pinned BoringSSL / `CRYPTO_set_ex_data` can fail a same-index replace
  or discard the stack after `take_ssl_ctx`.
- A rustls or other non-BoringSSL Gateway path stores handshake context
  without the current select short-circuit.
- Finalize store failure becomes injectable or shows up in production
  `ssl.log`.

## Resolution references

- `tls-ssl-ctx-owned-handoff-and-exactly-once-audit` — 2026-08-30 todo
  deferred from a file audit; 2026-09-19 closed with no code change.
  Secret-copy claim stale; finalize dual-emit accepted while unreachable.
  Originally `tasks/todo/tls-ssl-ctx-owned-handoff-and-exactly-once-audit.md`.
- Related: tls-F004 completion-fallback non-logging
  (`handshake-complete-fallback-no-double-log.md`).
