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

- `apply_edgion_tls_cert` strictly parses the full EdgionTls chain and compares
  the parser's leaf Unix bounds with the current Unix time.
- `apply_compiled_certificate_to_ssl` compares a Gateway listener certificate's
  cold-compiled Unix bounds with the current Unix time, then installs the immutable
  compiled chain/key. It performs no per-handshake PEM parsing or ASN.1 construction.

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

## Why the two implementations differ

The audit invariant and classifier are shared, while ownership differs.
EdgionTls still carries source material that its install path parses strictly;
the strict parser returns the Unix bounds used for the per-handshake check.
Gateway listener identities are validated and compiled on matcher rebuild, so
their install path compares the stored Unix bounds without repeating PEM/key
parsing. Both paths set the same fail-closed static category before binding an
invalid identity.

## Re-evaluation triggers

- A third cert-install entry point is added — re-check that it performs the validity check before
  binding an identity. Audit symmetry is the rule, not a property of these two functions.
- Either identity path changes its validity representation or classifier; the
  shared audit-signal invariant must be preserved.
- Compiled Gateway certificate ownership or its stored validity representation changes.

## Reference

- Issue: `tasks/tls/tls-F003-runtime-expiry-check-asymmetry.md` (closed by this fix)
- Related: F-010 (extract `install_cert_key_to_ssl` — deferred / accepted tradeoff, see
  [cert-key-install-helper-deferred.md](../intentional-design/cert-key-install-helper-deferred.md))
