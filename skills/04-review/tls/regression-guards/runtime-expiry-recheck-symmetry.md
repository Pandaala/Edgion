---
name: runtime-expiry-recheck-symmetry
description: Both certificate-install paths compare strict-parser Unix validity bounds at handshake time and emit the same static audit categories.
knowledge_type: regression-guards
---

# tls-F003 — Runtime expiry recheck is required on both cert-install paths

**Status:** ✅ fixed · 2026-05-27
**Severity:** Medium (BadLogic / asymmetric validation)

## Conclusion

`edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs` rechecks `not_before` / `not_after`
immediately before installing either serving identity, with source-specific mechanics:

Both paths compare cold-compiled leaf Unix bounds with the current Unix time and
then install an immutable compiled chain/key. Neither performs per-handshake PEM
parsing or ASN.1 construction:

- `apply_edgion_tls_cert` reads the bounds from the `CompiledServingCertificate`
  its `TlsStore` generation compiled (D0014).
- `apply_compiled_certificate_to_ssl` reads them from the Gateway listener
  certificate its matcher generation compiled.

Re-reports of the form "expiry recheck is redundant because BoringSSL rejects expired certs during
the handshake", "`apply_compiled_certificate_to_ssl` does no recheck while `apply_edgion_tls_cert` does
(asymmetry)", or "the `cert_expired` audit signal only fires for EdgionTls" are **not accepted** —
the asymmetry is resolved and the redundancy framing misses the point (see below).

## Why the recheck is intentional, not redundant

BoringSSL's own chain validation does cause the *client* to reject a genuinely expired server cert,
so the handshake fails either way. The recheck exists for **server-side observability**, not
correctness of the handshake outcome:

- It writes a static non-secret `err_log` category (`cert_expired` or
  `cert_not_yet_valid`) on `ssl_ctx.meta` that flows into tls.log and downstream
  alerting. Certificate dates are intentionally not embedded in diagnostics.
- Without it, operators see a generic handshake failure without knowing the configured cert has
  drifted past its `not_after` between controller rebuilds (rebuild cadence is operator-controlled,
  minutes to hours).
- The two install paths must emit the same audit signal — otherwise monitoring that keys off
  `cert_expired` only fires for one cert source.

## What was wrong (before the fix)

A Gateway listener certificate could previously be installed without a runtime validity recheck,
so a certificate that expired between matcher rebuilds produced no server-side `cert_expired`
audit entry. The current matcher compiles the certificate and its Unix validity bounds once; the
handshake path checks those bounds before atomic installation.

## Why the two implementations stayed separate

The audit invariant, the classifier, and now the compiled bounds representation
are all shared; both paths set the same fail-closed static category before
binding an invalid identity. The functions remain distinct only because their
surrounding policy differs (EdgionTls owns the resource's cipher and
`clientAuth` branches; the Gateway listener path owns dual-certificate selection
and `frontendValidation`).

**History:** before D0014, `apply_edgion_tls_cert` carried source PEM into the
handshake and derived its Unix bounds from a per-connection strict parse. That
asymmetry in *how* the bounds were obtained is gone; the audit symmetry it
guarded was already enforced and is unchanged.

## Re-evaluation triggers

- A third cert-install entry point is added — re-check that it performs the validity check before
  binding an identity. Audit symmetry is the rule, not a property of these two functions.
- Either identity path changes its validity representation or classifier; the
  shared audit-signal invariant must be preserved.
- Compiled Gateway certificate ownership or its stored validity representation changes.

## Reference

- Issue: `tasks/tls/tls-F003-runtime-expiry-check-asymmetry.md` (closed by this fix)
- Related: [edgion-tls-serving-chain-install.md](edgion-tls-serving-chain-install.md) —
  the cold-compiled EdgionTls identity and its shared atomic installer (D0014)
- Historical: F-010 (extract `install_cert_key_to_ssl`) was deferred in 2026-05-27 and is
  now retired, see
  [superseded/cert-key-install-helper-deferred.md](../superseded/cert-key-install-helper-deferred.md)
