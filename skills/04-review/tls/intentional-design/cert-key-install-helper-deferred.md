---
name: cert-key-install-helper-deferred
description: Use when reviewing proposals to merge the EdgionTls runtime PEM installer with the Gateway Listener cold-compiled atomic installer; explains why their distinct ownership remains intentional.
knowledge_type: intentional-design
---

# tls-F010 (cert/key install dup) — Shared `install_cert_key_to_ssl` extraction deferred

**Status:** ⚠️ accepted tradeoff (deferred) · 2026-05-27
**Severity:** Minor (Redundancy / maintenance only)

Note: distinct from the earlier `tls-F010` (frontendValidation `is_mtls` empty CA) in
[frontend-validation-is-mtls-empty-ca.md](../regression-guards/frontend-validation-is-mtls-empty-ca.md) —
that one fixed a code defect; this entry closes a refactor proposal from a later
review batch (same numeric ID, different finding).

## Conclusion

The historical proposal compared two per-handshake PEM installers. That shape
no longer exists. `apply_edgion_tls_cert` still receives source PEM at handshake
time and strictly parses the ordered chain and key before installing them on the
connection. Gateway Listener TLS is instead compiled on matcher rebuild and
`apply_compiled_certificate_to_ssl` atomically installs the immutable compiled
chain/key with `SSL_set_chain_and_key`.

A shared cert/key installation helper remains deferred because these are now
deliberately different ownership models rather than duplicated implementations.
Re-reports must evaluate the current source; the removed
`apply_secret_to_ssl` sequence is not current evidence.

## Why the extraction is deferred (not refused)

The two paths still share high-level serving-identity invariants, but a common
installation helper does not fit their current ownership:

- Audit symmetry is explicit: both paths compare strict-parser-derived Unix
  validity bounds with the shared inclusive classifier and emit the same static
  `cert_expired` / `cert_not_yet_valid` categories.
- EdgionTls must parse all source material before binding, install leaf then
  intermediates then key, and clear inherited or partial SSL identity on
  failure. Gateway TLS owns cold compilation and one atomic install operation.
- Combining those flows would either move EdgionTls parsing into an installer
  misleadingly shared with the cold path or weaken the Gateway path's
  allocation-free handshake contract.
- Per-arm static diagnostics and fail-close clearing order are security and
  audit contracts, not incidental boilerplate.

The same reasoning class as `plugins-http-F004` (rate-limit header injection,
declined despite syntactic similarity) and `plugins-stream-F003`
(`ValidateAndInit` trait, declined): syntactic duplication alone does not
justify a shared helper when (a) the diverging context lives outside the
abstraction and (b) no concrete drift trigger is pending.

## What this is NOT

This entry does **not** sanction semantic drift. Both serving paths must retain
complete ordered-chain delivery, fail-closed identity replacement, and runtime
validity/audit symmetry. Their mechanisms need not be textually identical:
EdgionTls parses and installs on the connection, while Gateway TLS compiles on
the cold path and installs atomically.

## Decision Rule

When a review/audit report claims any of the following, close as "accepted
tradeoff (deferred)" referencing this entry:

1. "The EdgionTls and Gateway installers should share one helper" → **deferred**;
   one consumes PEM per handshake and the other consumes a cold-compiled atomic
   identity.
2. "The runtime expiry check should move into a shared installer" → **deferred**;
   share the Unix-bound classifier and audit categories, not incompatible
   identity ownership.
3. "The static `err_log` arms should expose backend errors" → **rejected**;
   fixed non-secret categories are intentional.

## Re-evaluation triggers (re-open under a new ID, not this one)

- A third cert-install entry point is added (e.g. a future per-route TLS
  provider, an out-of-band cert hot-load path) — three call sites flip the
  cost/benefit on extraction.
- EdgionTls gains a cold compiled identity owned by its publication generation;
  that is tracked separately as D0014 and is not part of this serving-chain fix.
- A backend-independent compiled identity abstraction becomes available to both
  publication paths without adding handshake-time parsing or allocation.
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
  - `edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs::apply_edgion_tls_cert`
  - `edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs::apply_compiled_certificate_to_ssl`
- Related rules:
  - [runtime-expiry-recheck-symmetry.md](../regression-guards/runtime-expiry-recheck-symmetry.md) —
    audit-signal symmetry on both install paths (already enforced)
  - [cert-matcher-duplication-accepted.md](cert-matcher-duplication-accepted.md) —
    same class of decline (structural similarity, divergent per-site context)

## Resolution references

- `tls-F010-dup` — 2026-05-27 (⚠️ accepted tradeoff, deferred).
