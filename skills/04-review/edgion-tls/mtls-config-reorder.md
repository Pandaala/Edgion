---
name: mtls-config-reorder
description: mTLS verify-setup is now configured BEFORE cert/key in both apply_edgion_tls_cert and apply_gateway_tls_cert, so a configure_mtls or configure_gateway_frontend_mtls Err leaves no cert bound and BoringSSL must fail the handshake. Reject re-reports claiming a Mutual mTLS listener silently downgrades to "anonymous accepted" when the CA Secret is missing or the CA PEM is malformed.
---

# tls-F011 — mTLS verify-setup is now fail-closed (no silent "anonymous accepted" downgrade)

**Status:** ✅ fixed · 2026-05-27
**Severity:** High (BadLogic) — silent bypass of an operator-required security boundary

## Conclusion

Both `apply_edgion_tls_cert` and `apply_gateway_tls_cert`
(`edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs`) now configure mTLS
verification **before** binding cert/key to the SSL. When `configure_mtls`
returns Err (CA Secret missing, garbage CA PEM, store build failure),
`apply_edgion_tls_cert` stamps `err_log = Some(format!("mtls config: {e}"))`
and returns; cert/key have not yet been bound, BoringSSL has no cert to serve,
and the handshake aborts. The same applies on the Gateway-listener
`frontendValidation` path with `err_log = "frontend mTLS error: ..."`.

Re-reports of the form "Mutual mTLS silently downgrades to anonymous accepted
when caSecretRef points to a missing Secret", "configure_mtls Err is fail-open",
"add_ca_bundle_to_store failure does not block handshake", or "frontendValidation
garbage CA PEM completes handshake without requesting client cert" are **not
accepted** — none is still true.

## Why an `err_log + return` pattern alone would have been insufficient

`TlsAccept::certificate_callback` is a `()`-returning async fn that always
reports success to BoringSSL, so the only handshake-aborting effect available
from inside cert_cb is "do not bind a cert/key". Before the fix, the mTLS
verify-setup branch ran AFTER `ssl_use_certificate` / `ssl_use_private_key`
(EdgionTls path) or AFTER `apply_secret_to_ssl` (Gateway path) — so by the
time `configure_mtls` Err'd, BoringSSL already had a usable cert/key on the
SSL. The `err_log + return` only skipped `finalize_ssl_ctx`; the handshake
continued, completing with no client cert ever requested and `is_mtls`
silently left at its default.

The reorder above is what actually closes the gap. This is the same rule the
cipher branch was retrofitted with in [cipher-fail-closed]; F011 is
its second application.

## What was wrong (before the fix)

`apply_edgion_tls_cert`'s body ended with:

```rust
if let Err(e) = pingora_core::tls::ext::ssl_use_certificate(ssl, &cert) { ... return; }
// ... key read + ssl_use_private_key ...
if let Some(ref client_auth) = edgion_tls.spec.client_auth {
    if let Err(e) = self.configure_mtls(ssl, client_auth, edgion_tls) {
        ssl_ctx.meta.err_log = Some(format!("mtls config: {e}"));
        return; // <-- too late, cert+key already bound
    }
}
```

Reachable Err points that all silently failed-open:

| Failure source | Reason | Production-realistic? |
|---|---|---|
| `edgion_tls.ca_cert_pem()` Err | `caSecretRef` typo / namespace mismatch / Secret-resolver race | Yes |
| `add_ca_bundle_to_store` Err | rotation emitted empty / malformed CA bundle | Yes |
| `X509StoreBuilder::new()` Err | BoringSSL allocation failure | Rare |
| `set_verify_cert_store` Err | BoringSSL internal | Rare |
| `set_mtls_verify_callback` Err | allocation failure | Rare |

`apply_gateway_tls_cert` had the same shape with
`configure_gateway_frontend_mtls` running after `apply_secret_to_ssl`. The
Gateway path additionally interacts with [frontend-mtls-empty-store-on-missing-ca-key],
which fixed the orthogonal "every Secret lacks `ca.crt` ⇒ empty store" leg; F011
covers the leg where `ca.crt` is present but its PEM is malformed.

Controller admission (`handlers/edgion_tls.rs::validate`) does not gate
`client_auth.ca_secret_ref` existence — a misconfigured CR reaches the
gateway, and is the realistic production trigger.

## Why the fix is correct

mTLS verify configuration is a security-boundary control on par with cipher
policy (tls-F002) — strictly worse, in fact: the bypassed control is
"operator required client cert" rather than "operator restricted cipher
suite". The function-level doc comments on both `apply_*_cert` functions now
state the ordering rule explicitly: every SSL-level setup without a cert/key
dependency must precede cert/key binding, because cert_cb has no other
handshake-aborting lever. `configure_mtls` and `configure_gateway_frontend_mtls`
operate only on client-side verify state and qualify; future security-boundary
branches added to these functions MUST be placed before cert/key as well.

If a future change adds a server cert read inside either function (e.g.
SAN-mirror logic), the ordering decision must be revisited.

## Tests guarding this

- `edgion-tests/src/client/suites/edgion_tls/mtls_failclose/mtls_failclose.rs::test_mtls_ca_missing_failclosed`
  drives the EdgionTls path with `caSecretRef` to a Secret that does not
  exist, asserts openssl s_client handshake aborts (`Cipher=(NONE)`), and
  cross-checks `ssl.log` for an `err_log` entry with prefix `mtls config:`
  and no `handshake_complete_time` field.
- `..::test_mtls_ca_garbage_pem_failclosed` drives the EdgionTls path with
  `ca.crt = 'not a pem'`, asserting the same shape via the
  `add_ca_bundle_to_store` Err leg.
- `..::test_frontend_mtls_garbage_ca_pem_failclosed` drives the Gateway
  listener `frontendValidation` path with a garbage `ca.crt`, asserting
  the handshake aborts and `ssl.log` `err_log` starts with `frontend mTLS error:`.

Pre-fix, all three tests would have failed (handshake completed with a real
cipher); the reorder is what flips them green.

## Relationship to siblings

- [cipher-fail-closed] established the ordering rule on the cipher
  branch. F011 is its second application.
- [frontend-mtls-empty-store-on-missing-ca-key] covers an orthogonal
  Gateway-side sub-symptom (`certs_added == 0` when every referenced Secret
  lacks the `ca.crt` key). F011 covers the leg where `ca.crt` is present but
  malformed; both legs now fail-closed.

## Out-of-scope follow-up (not done in this fix)

Controller-side admission validation for `client_auth.ca_secret_ref` /
listener `frontendValidation.caCertificateRefs` Secret existence and PEM
parseability would let admission reject typos with a clear status condition
instead of failing per-connection. It is independent of the fail-closed
contract above; if added later, this skill entry stays valid (the runtime
invariant is still required for defense in depth, e.g. a Secret going stale
after admission already succeeded).
