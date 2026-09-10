---
name: frontend-validation-is-mtls-empty-ca
description: configure_gateway_frontend_mtls returns Ok(false) only for NotConfigured; strict empty or incomplete CA projections are StrictReject, while usable modes return Ok(true). Reject re-reports of an unconditional is_mtls stamp.
knowledge_type: regression-guards
---

# tls-F010 — Gateway `frontendValidation` mislabeled `is_mtls = true` with empty CA refs

**Status:** ✅ fixed · 2026-05-21
**Severity:** Low (Observability) — log/plugin-context label disagreed with the actual handshake; the handshake itself was a correct one-way TLS handshake

## Conclusion

`apply_gateway_tls_cert` (`edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs`) stamps
`ssl_ctx.meta.is_mtls` from `configure_gateway_frontend_mtls`'s `Result<bool, _>`. `Ok(false)`
is reserved for `FrontendMtlsOutcome::NotConfigured`. `StrictEffective` and
`InsecureFallback` install a verify mode and return `Ok(true)`; `StrictReject` returns `Err`.
The matcher compiles strict empty, missing, incomplete, or invalid CA projections into
`StrictReject`, so they cannot be mislabeled as intentional one-way TLS.

Re-reports of the form "`apply_gateway_tls_cert` sets `is_mtls = true` even when no verify mode
was installed" are **not accepted** — verify both the caller's bool assignment and the compiled
frontend outcome. Do not treat a strict empty-reference projection as `Ok(false)`.

## What was wrong (before the fix)

Before the standard frontend projection existed, a controller-private listener
`tls.frontendValidation` value with empty refs could reach the data plane without installing a
verify mode, while the caller still marked the connection as mTLS. The only operator-facing path
is now `Gateway.spec.tls.frontend.default/perPort`; the controller projects it into the
schema-hidden listener field. Empty or incomplete strict projections compile to a matched
`StrictReject` entry, while an explicitly absent standard validation remains ordinary one-way TLS.

## Why the fix is correct

`is_mtls` should mean "client-cert verification was actually requested for this connection",
not "the frontend-mTLS setup call returned Ok". Returning a bool that is `true` only on the
verify-mode-installed path makes the flag track the real handshake. This mirrors the sibling
EdgionTls path, which tls-F001 already changed to `is_mtls = et.is_mtls_enabled()` (Terminate
mode installs no verify mode → not flagged). Both mTLS paths in this file now key `is_mtls`
off whether a verify mode was installed, not off an `Ok` return.

## Fix suggestions not accepted

- "Treat strict empty CA refs like absent frontend validation" — rejected; operator intent is
  strict client authentication, so the matcher publishes `StrictReject`.

## Re-evaluation triggers

- A new caller of `configure_gateway_frontend_mtls` ignores the returned bool.
- The frontend outcome enum or verify-mode contract changes.

## Reference cases

- tls-F001 (`terminate-mtls-ca-ordering.md`) — sibling EdgionTls path; same
  "key the flag/behavior off the real verify mode, not an Ok return" class.
