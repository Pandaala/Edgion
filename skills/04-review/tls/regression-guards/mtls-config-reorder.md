---
name: mtls-config-reorder
description: mTLS verify-setup is now configured BEFORE cert/key in both apply_edgion_tls_cert and apply_gateway_tls_cert, so a configure_mtls or configure_gateway_frontend_mtls Err leaves no cert bound and BoringSSL must fail the handshake. Reject re-reports claiming a Mutual mTLS listener silently downgrades to "anonymous accepted" when the CA Secret is missing or the CA PEM is malformed.
knowledge_type: regression-guards
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
and the handshake aborts. The same runtime defense applies on the
Gateway-listener `frontendValidation` path with
`err_log = "frontend mTLS error: ..."`. The Controller now validates resolved
frontend CA material first, marks a listener with malformed CA PEM as
`Programmed=False`, and the listener connection gate rejects it before TLS
callbacks run. The callback ordering remains required as defense in depth for
any strict rejection that reaches the TLS matcher.

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
(EdgionTls path) or after the then-current Gateway Secret installer — so by the
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

| Failure source | Evaluated | Reason | Production-realistic? |
|---|---|---|---|
| `edgion_tls.ca_cert_pem()` Err | publish | `caSecretRef` typo / namespace mismatch / Secret-resolver race | Yes |
| `add_ca_bundle_to_store` Err | publish | rotation emitted empty / malformed CA bundle | Yes |
| `X509StoreBuilder::new()` Err | publish | BoringSSL allocation failure | Rare |
| `set_verify_cert_store` Err | handshake | BoringSSL internal | Rare |
| `set_mtls_verify_callback` Err | handshake | allocation failure | Rare |

The `Evaluated` column is current state, not the state at the time of the fix:
since D0014 the first three sources are resolved when `TlsStore` publishes an
accepted generation (`compile_client_ca_store`), not per handshake. Their error
text is carried unchanged on `CompiledEdgionTls.client_ca_store`, and
`configure_mtls` still stamps the same `mtls config: …` audit entry and returns
before binding cert/key — the fail-close shape this page guards is unaffected by
that timing move.

`apply_gateway_tls_cert` had the same shape with
`configure_gateway_frontend_mtls` running after the then-current Gateway Secret
installer. That installer has since been removed: the current Gateway path
configures frontend mTLS and FIPS policy before
`apply_compiled_certificate_to_ssl` binds the selected identity through
`install_compiled_serving_identity`. The EdgionTls path configures FIPS policy
and `configure_mtls` before using the same identity installer. D0015 revalidated
these ownership and ordering statements against the current BoringSSL callback;
it does not change the historical failure mechanism. The
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
  and `test_mtls_ca_garbage_pem_failclosed` retain their original policy cases.
  ARCH-283 replaces their OpenSSL incomplete-field/watchdog acceptance with a
  bounded direct TLS 1.2 client and exact typed `HandshakeFailure`. Setup,
  connection refusal, deadline, arbitrary peer close and unrelated alerts fail.
  Both targets and a same-listener, same-serving-Secret Terminate control must
  have resolved attachment publication at Controller and Gateway; the control
  must complete with the pinned serving certificate. Local cases additionally
  require fresh `mtls config:` evidence without `handshake_complete_time`, matching
  exact SNI, listener port and selected EdgionTls namespace/name. A retained file
  offset and epoch-millisecond connection timestamp fence exclude stored history
  and delayed pre-probe logger records; rotation/truncation and missing evidence
  fail. No OpenSSL child remains on this scoped path.
- Kubernetes runs use typed wire rejection plus exact attachment publication,
  positive serving control and local/Kubernetes fixture-policy guards. They do
  not read an unavailable host ssl.log and do not claim direct callback-cause
  audit. Projection proves published attachment, not that a particular callback
  emitted the alert. Missing-SNI rejection can share `HandshakeFailure`, so a
  bare alert or a sibling positive alone is insufficient.
- `test_frontend_mtls_garbage_ca_pem_failclosed` retains authenticated polling of
  `mtls-failclose-gateway` / `https-frontend-mtls-failclose` for
  `ResolvedRefs=False/InvalidCACertificateRef` and `Programmed=False/Invalid`.
  Its connection must reach TCP and close promptly before TLS; refusal,
  deadline, unrelated alert or completed handshake fails. Supporting
  `fv_invalid_ca_ref_not_served` uses the same evidence contract for its own
  `invalid-ca-cert-gw` / `https-invalid-ca` listener. No ssl.log entry is required
  because these connection gates reject before TLS callbacks run.

ARCH-283's scoped probe/decision tests are network-free. Fresh local and
Kubernetes wire runs are separate verification claims; this regression-boundary
description does not assert either has run. See the owning
[negative-evidence contract](../../testing/regression-guards/mandatory-integration-gates-fail-closed.md#negative-tls-policy-evidence).

Before the runtime reorder, a malformed EdgionTls CA could complete a handshake
with a real cipher. The standard Gateway path now has the additional Controller
admission and connection-gate protection described above.

## Relationship to siblings

- [cipher-fail-closed] established the ordering rule on the cipher
  branch. F011 is its second application.
- [frontend-mtls-empty-store-on-missing-ca-key] covers an orthogonal
  Gateway-side sub-symptom (`certs_added == 0` when every referenced Secret
  lacks the `ca.crt` key). F011 covers the leg where `ca.crt` is present but
  malformed; both legs now fail-closed.

## Out-of-scope follow-up (not done in this fix)

Controller-side admission validation for `client_auth.ca_secret_ref` /
standard `Gateway.spec.tls.frontend` CA-reference existence and PEM
parseability would let admission reject typos with a clear status condition
instead of failing per-connection. It is independent of the fail-closed
contract above; if added later, this skill entry stays valid (the runtime
invariant is still required for defense in depth, e.g. a Secret going stale
after admission already succeeded).
