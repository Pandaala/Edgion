---
name: runtime-expiry-recheck-symmetry
description: Both cert-install paths (apply_edgion_tls_cert and apply_secret_to_ssl) now perform the same runtime expiry recheck before ssl_use_certificate. Reject re-reports framing this check as redundant.
---

# tls-F003 — Runtime expiry recheck is required on both cert-install paths

**Status:** ✅ fixed · 2026-05-27
**Severity:** Medium (BadLogic / asymmetric validation)

## Conclusion

`edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs` now performs an `Asn1Time::days_from_now(0)`
recheck of `not_before` / `not_after` immediately after `X509::from_pem` in **both** cert-install
entry points:

- `apply_edgion_tls_cert` — for `EdgionTls`-sourced certs (pre-existing).
- `apply_secret_to_ssl` — for `GatewayTls` listener certs sourced from K8s `Secret`s (added by F-003).

The check is gated by `#[cfg(feature = "boringssl")]`; the non-boring backend keeps the legacy
install-without-recheck behavior (no `Asn1Time::days_from_now` shim wired up there).

Re-reports of the form "expiry recheck is redundant because BoringSSL rejects expired certs during
the handshake", "`apply_secret_to_ssl` does no recheck while `apply_edgion_tls_cert` does
(asymmetry)", or "the `cert_expired` audit signal only fires for EdgionTls" are **not accepted** —
the asymmetry is resolved and the redundancy framing misses the point (see below).

## Why the recheck is intentional, not redundant

BoringSSL's own chain validation does cause the *client* to reject a genuinely expired server cert,
so the handshake fails either way. The recheck exists for **server-side observability**, not
correctness of the handshake outcome:

- It writes a structured `err_log` (`cert_expired: not_after=…` / `cert_not_yet_valid: not_before=…`)
  on `ssl_ctx.meta` that flows into tls.log and downstream alerting.
- Without it, operators see a generic handshake failure without knowing the configured cert has
  drifted past its `not_after` between controller rebuilds (rebuild cadence is operator-controlled,
  minutes to hours).
- The two install paths must emit the same audit signal — otherwise monitoring that keys off
  `cert_expired` only fires for one cert source.

## What was wrong (before the fix)

`apply_secret_to_ssl` parsed the PEM at the `X509::from_pem` site, then called
`pingora_core::tls::ext::ssl_use_certificate` immediately, with no `Asn1Time` comparison in between.
All expiry-related symbols (`Asn1Time`, `days_from_now`, `not_after`, `not_before`) appeared only
inside the `apply_edgion_tls_cert` block, so a `GatewayTls` listener whose `Secret` cert expired
between rebuilds would be installed onto the SSL slot and presented to clients on every subsequent
handshake until the next resync, silently — no `cert_expired` audit entry.

## Why the fix is minimal

The fix is a textual mirror of the existing EdgionTls block:

- Same `#[cfg(feature = "boringssl")]` gate.
- Same `Asn1Time::days_from_now(0)` construction.
- Same `not_after` / `not_before` comparisons against `now` (RFC 5280 inclusive bounds).
  The new block uses `< now` / `> now` while the EdgionTls block still uses `< &now` /
  `> &now`; semantically identical (`Asn1Time` and `&Asn1TimeRef` cross-compare via
  `PartialOrd` either way). The `&` was dropped on the new block to silence
  `clippy::op_ref` — the EdgionTls block's pre-existing lint is left alone (out of F-003 scope).
- Same `err_log; return` failure mode (fail-closed — do not install an expired cert).
- Same `cert_expiry_check_failed` log on `days_from_now` allocation failure.

Extracting a shared helper (`install_cert_key_to_ssl`, F-010) is deliberately *not* part of F-003 —
F-003 only restores audit symmetry. The helper extraction has since been deferred (see
[cert-key-install-helper-deferred.md](cert-key-install-helper-deferred.md)) and
will collapse both blocks only when a re-evaluation trigger fires.

## Re-evaluation triggers

- A third cert-install entry point is added — re-check that it also performs the recheck before
  `ssl_use_certificate`. Audit symmetry is the rule, not a property of these two functions.
- F-010 lands and the helper is extracted — the two cfg-gated blocks should collapse into one; the
  audit-signal invariant must be preserved.
- The `cfg(not(feature = "boringssl"))` rustls backend wires up an equivalent expiry primitive —
  then both backends should perform the recheck on both paths (drop the `cfg` gate).

## Reference

- Issue: `tasks/tls/tls-F003-runtime-expiry-check-asymmetry.md` (closed by this fix)
- Related: F-010 (extract `install_cert_key_to_ssl` — deferred / accepted tradeoff, see
  [cert-key-install-helper-deferred.md](cert-key-install-helper-deferred.md))
