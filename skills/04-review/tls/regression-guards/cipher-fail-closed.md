---
name: cipher-fail-closed
description: spec.ciphers is now configured BEFORE cert/key in apply_edgion_tls_cert, so a configure_ciphers Err leaves no cert bound and BoringSSL must fail the handshake. Reject re-reports claiming spec.ciphers silently falls through to BoringSSL defaults on an unrecognized name.
knowledge_type: regression-guards
---

# tls-F002 — cipher config failure is now fail-closed (no silent default-suite fallback)

**Status:** ✅ fixed · 2026-05-27 (re-fixed by reordering, same date)
**Severity:** High (BadLogic) — silent bypass of an operator-set security boundary

## Conclusion

`apply_edgion_tls_cert` (`edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs`) now
processes `spec.ciphers` **before** binding cert/key to the SSL. When
`configure_ciphers` returns `Err`, the function stamps
`ssl_ctx.meta.err_log = Some(format!("cipher config: {e}"))` and `return`s; because
`ssl_use_certificate` / `ssl_use_private_key` have not yet run, BoringSSL has no cert to
serve and the handshake aborts. The `certificate_callback` gate (`:282-285`) sees
`err_log.is_some()` and writes the real rejection reason to the audited `ssl.log`.

Re-reports of the form "spec.ciphers silently falls through to BoringSSL default suite",
"cipher config failure does not block handshake", or "rejected cipher names never surface
in ssl.log" are **not accepted** — none is still true.

## Why the first fix attempt was insufficient

The original "fix" kept the cipher branch in its historical position after
`ssl_use_certificate` / `ssl_use_private_key`, and relied on the same
`err_log + return` pattern the cert/key branches use. That pattern only fails-closed when
cert/key are not yet bound: by the time the cipher branch ran, BoringSSL already had a
valid cert and key on the SSL, so returning from cert_cb (with `err_log` set) only skipped
`finalize_ssl_ctx` — it did not prevent BoringSSL from completing the handshake on its
inherited default suite. The `EdgionTls_cipher::cipher_bad_name_failclosed` integration
test caught exactly this: handshake completed with `Cipher=ECDHE-RSA-CHACHA20-POLY1305`
even though `ssl.log` recorded `err_log="cipher config: rejected ciphers: …"`.

The reorder above is what actually closes the gap — pingora's
`TlsAccept::certificate_callback` is a `()`-returning async fn and always reports success
to BoringSSL, so the only fail-close lever from inside cert_cb is "do not bind a cert/key".

## What was wrong (before the fix)

The cipher branch was the only TLS-config branch in `apply_edgion_tls_cert` that did not
fail-closed:

```rust
if let Some(ref ciphers) = edgion_tls.spec.ciphers {
    if let Err(_e) = self.configure_ciphers(ssl, ciphers) {
        let _ = ssl_ctx.meta.log
            .get_or_insert_with(LogBuffer::new)
            .push_err("cipher config fail");
    }
}
```

Three things compounded into a silent security regression:

1. The `Err(_e)` was discarded — `configure_ciphers` had carried
   `format!("rejected ciphers: {cipher_list}")` for diagnostics; it never reached anything
   that gets persisted.
2. The fallback wrote to the per-connection `meta.log` buffer, not `err_log`. Only
   `err_log` is checked by `certificate_callback` to decide fail-closed, so the function
   fell through to `finalize_ssl_ctx` and the handshake completed normally.
3. BoringSSL `SSL_set_strict_cipher_list` is designed to **reject** unrecognized cipher
   strings rather than apply them, and on rejection it leaves the SSL on its inherited
   default cipher list (the SSL_CTX default = BoringSSL's full default suite). Operators
   who set `spec.ciphers` precisely to enforce a restrictive policy (FIPS-only, exclude
   weak/legacy) had that restriction silently dropped on any cipher-name typo, OpenSSL-only
   alias, or BoringSSL version-bump rename — and the listener accepted connections
   negotiating from the unrestricted default suite. The only signal was a per-connection
   "cipher config fail" note in `ssl.log` with no actual reason attached.

No earlier gate caught this: the controller `validate()` for `EdgionTls`
(`edgion-controller/src/conf_mgr/.../handlers/edgion_tls.rs`) checks only `parent_refs` and
cert/key matching — it does no cipher-name validation, and the CRD field is a free-form
`Option<Vec<String>>`.

## Why the fix is correct

Cipher configuration is a security-boundary control on par with cert/key/mTLS. The
sibling branches that fail-closed correctly do so by happening before any usable SSL
state is bound — `cert_pem` / `bad cert` / `set cert` all return before
`ssl_use_certificate` succeeds, and `key_pem` / `bad key` / `set key` leave the SSL
without a matching key (BoringSSL rejects). Aligning cipher with this invariant — by
moving it to the first non-trivial step of the function — restores the rule that every
TLS-config failure in the per-connection cert callback aborts the handshake with an
audited reason, and matches the secure default for operator-enforced policy controls.

The function-level doc comment on `apply_edgion_tls_cert` now states this ordering rule
explicitly: every SSL-level setup without a cert/key dependency must precede cert/key
binding. `configure_ciphers` qualifies (pure `SSL_set_strict_cipher_list`); future
security-boundary branches added to this function MUST be placed before cert/key as well.

If a future "graceful degrade on cipher mismatch" mode is wanted it must be an explicit
opt-in (e.g. an `EdgionTls.spec.cipherPolicy: strict | bestEffort` field), not the default.
Until then, treat the strict semantics as load-bearing.

The same ordering rule has since been applied to the mTLS verify-setup
branches in `apply_edgion_tls_cert` and `apply_gateway_tls_cert`; see
[mtls-config-reorder] for the second application.

## Tests guarding this

- `edgion-tests/src/client/suites/edgion_tls/cipher/cipher.rs::test_bad_cipher_name_failclosed`
  drives a `cipher-badname` EdgionTls (`edgion-tests/integration/conf/EdgionTls/cipher/EdgionTls_cipher_badname.yaml`)
  with an obviously-invalid cipher name and asserts the openssl `s_client` handshake does
  NOT complete. Before the fix this test would have failed (handshake completed with a
  default-suite cipher).
- The pre-existing legacy / modern positive cases still pass: valid cipher names continue
  to be applied through the same path.

## Out-of-scope follow-up (not done in this fix)

Controller-side cipher-name validation in `EdgionTls::validate()` would let admission
reject typos with a clear status condition instead of failing per-connection at handshake
time. It is independent of the fail-closed contract above; if added later, this skill
entry stays valid (the runtime invariant is still required for defense in depth, e.g. a
BoringSSL upgrade dropping a previously-known cipher name after admission already
succeeded).
