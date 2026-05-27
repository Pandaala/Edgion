---
name: tls-F010-cert-key-install-helper-deferred
description: Use when reviewing findings that flag the duplicated cert/key parse-install boilerplate across `apply_edgion_tls_cert` and `apply_secret_to_ssl` and propose extracting an `install_cert_key_to_ssl` helper; explains why the extraction is deferred and what conditions would re-open it.
---

# tls-F010 (cert/key install dup) — Shared `install_cert_key_to_ssl` extraction deferred

**Status:** ⚠️ accepted tradeoff (deferred) · 2026-05-27
**Severity:** Minor (Redundancy / maintenance only)

Note: distinct from the earlier `tls-F010` (frontendValidation `is_mtls` empty CA) in
[tls-F010-frontend-validation-is-mtls-empty-ca.md](tls-F010-frontend-validation-is-mtls-empty-ca.md) —
that one fixed a code defect; this entry closes a refactor proposal from a later
review batch (same numeric ID, different finding).

## Conclusion

`apply_edgion_tls_cert` and `apply_secret_to_ssl` in
`src/core/gateway/tls/runtime/gateway/tls_pingora.rs` each carry their own
`X509::from_pem` → boringssl runtime expiry recheck → `ssl_use_certificate` →
`PKey::private_key_from_pem` → `ssl_use_private_key` sequence, with matching
`err_log; return` failure handling. The proposal to extract a shared private
helper (e.g. `install_cert_key_to_ssl(&mut SslRef, cert_pem, key_pem, &mut SslCtx) -> bool`)
is **not accepted as a fix-issue task** at this time. Re-reports of the
structural-duplication form should be closed by referencing this entry.

## Why the extraction is deferred (not refused)

The duplication is real and the helper is a sensible target, but the current
cost/benefit does not justify acting now:

- Audit symmetry is already restored. After tls-F003 both paths perform the same
  `Asn1Time::days_from_now(0)` recheck with the same `cert_expired` /
  `cert_not_yet_valid` audit strings — the live drift risk this finding warned
  about is already neutralized.
- The two callers differ in PEM acquisition (`EdgionTls` accessor vs Secret
  UTF-8 decode); the install body itself is the only shared part. A helper would
  collapse ~35 lines across the two functions but leave the divergent prelude
  in each caller — the per-site readability win is moderate.
- No active feature work is pending on this code path (chain certs, stricter
  key validation, new install entry points). Without a concrete trigger, the
  refactor would change a load-bearing handshake function purely for line-count
  reduction.
- The risk of getting the err-string contract subtly wrong during extraction
  (string-equal log assertions, ordering of `err_log; return` vs
  `pingora_core::tls::ext::*` Err paths) outweighs the maintenance win as long
  as the two sites are stable.

The same reasoning class as `plugins-http-F004` (rate-limit header injection,
declined despite syntactic similarity) and `plugins-stream-F003`
(`ValidateAndInit` trait, declined): syntactic duplication alone does not
justify a shared helper when (a) the diverging context lives outside the
abstraction and (b) no concrete drift trigger is pending.

## What this is NOT

This entry does **not** sanction adding new asymmetry between the two paths:

- Any future change to the install body (chain certs, additional key checks,
  new err_log fields) MUST be applied to BOTH `apply_edgion_tls_cert` and
  `apply_secret_to_ssl` in the same edit; landing it on one side alone is the
  scenario this entry expects to be caught by the next review pass.
- The runtime expiry recheck on both sides (per tls-F003) is mandatory; the
  two cfg-gated blocks must continue to emit the same audit signal.

## Decision Rule

When a review/audit report claims any of the following, close as "accepted
tradeoff (deferred)" referencing this entry:

1. "`apply_edgion_tls_cert` and `apply_secret_to_ssl` duplicate the cert/key
   parse-install sequence — extract a shared `install_cert_key_to_ssl` helper"
   → **deferred**; the live drift risk (runtime expiry) is already resolved by
   tls-F003; no active feature work needs the abstraction.
2. "The runtime expiry recheck block is duplicated across the two install
   paths and should live in a helper" → **deferred**; the duplication is
   intentional under this entry until a re-evaluation trigger fires.
3. "The `err_log` failure-arm pattern is repeated across the two functions" →
   **deferred**; the per-arm strings are part of the audit/log contract and
   the cost of preserving them through extraction without drift is the reason
   the helper is not yet warranted.

## Re-evaluation triggers (re-open under a new ID, not this one)

- A third cert-install entry point is added (e.g. a future per-route TLS
  provider, an out-of-band cert hot-load path) — three call sites flip the
  cost/benefit on extraction.
- Chain-certificate support is added — the install body grows beyond a single
  `ssl_use_certificate` call and the per-site duplication compounds.
- Stricter private-key validation is added on either side (e.g. minimum key
  size, algorithm allowlist) — first such addition must be cloned to the other
  path AND should be the trigger to land the helper.
- A real incident traces back to a semantic change applied to one install path
  but not the other — the comments + this entry are the current guardrail; an
  incident means the guardrail failed and the helper is now warranted.
- The `cfg(not(feature = "boringssl"))` rustls backend wires up an equivalent
  expiry primitive — both backends should then perform the recheck on both
  paths and the cfg gate can drop; consolidating into a helper at the same time
  is the natural shape.

## Reference

- Issue: `tasks/tls/tls-F010-cert-key-install-boilerplate-duplicated.md`
  (closed as accepted tradeoff (deferred) 2026-05-27)
- Code:
  - `src/core/gateway/tls/runtime/gateway/tls_pingora.rs::apply_edgion_tls_cert`
  - `src/core/gateway/tls/runtime/gateway/tls_pingora.rs::apply_secret_to_ssl`
- Related rules:
  - [tls-F003-runtime-expiry-recheck-symmetry.md](tls-F003-runtime-expiry-recheck-symmetry.md) —
    audit-signal symmetry on both install paths (already enforced)
  - [tls-F007-cert-matcher-duplication-accepted.md](tls-F007-cert-matcher-duplication-accepted.md) —
    same class of decline (structural similarity, divergent per-site context)
