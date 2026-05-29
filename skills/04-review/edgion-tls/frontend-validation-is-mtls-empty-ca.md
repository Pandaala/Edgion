---
name: frontend-validation-is-mtls-empty-ca
description: configure_gateway_frontend_mtls now returns a bool and the caller stamps is_mtls from it, so a frontendValidation listener with empty/None CA refs no longer mislabels is_mtls=true. Reject re-reports of the unconditional is_mtls=true stamp.
---

# tls-F010 — Gateway `frontendValidation` mislabeled `is_mtls = true` with empty CA refs

**Status:** ✅ fixed · 2026-05-21
**Severity:** Low (Observability) — log/plugin-context label disagreed with the actual handshake; the handshake itself was a correct one-way TLS handshake

## Conclusion

`apply_gateway_tls_cert` (`src/core/gateway/tls/runtime/gateway/tls_pingora.rs`) no longer
stamps `ssl_ctx.meta.is_mtls = true` unconditionally on an `Ok` return from
`configure_gateway_frontend_mtls`. The callee now returns `Result<bool, _>`: `Ok(false)` on
the empty/None-CA-refs early return (no verify mode installed, no client cert requested) and
`Ok(true)` only after `ssl.set_verify(...)` actually runs. The caller stamps `is_mtls` from
that bool.

Re-reports of the form "`apply_gateway_tls_cert` sets `is_mtls = true` even when
`configure_gateway_frontend_mtls` installed no verify mode", "frontendValidation with empty
`caCertificateRefs` reports mTLS while `extract_client_cert_info` returns None", or
"is_mtls label and client-cert data disagree on the Gateway frontend path" are **not
accepted** — verify the caller stamps `is_mtls` from the callee's bool before re-reporting.

## What was wrong (before the fix)

A Gateway listener with `tls.frontendValidation: Some` but `caCertificateRefs: None`/`Some([])`
is **not** fenced by the controller: `has_all_ca_refs_invalid`
(`controller/.../handlers/gateway.rs`) returns `false` for zero refs, so the listener is
`Accepted=True` and reaches the data plane with `resolved_frontend_ca_secrets = None`.
`configure_gateway_frontend_mtls` then early-returned `Ok(())` without calling `set_verify`,
yet the caller's `else` arm set `is_mtls = true`. The flag propagated to `tls.log` and the
`tls_route` plugin context (`ctx.is_mtls`) while `extract_client_cert_info` returned `None`
(no peer cert was ever requested) — the label and the data disagreed, and a custom tls_route
plugin keying off `is_mtls` would be misled.

## Why the fix is correct

`is_mtls` should mean "client-cert verification was actually requested for this connection",
not "the frontend-mTLS setup call returned Ok". Returning a bool that is `true` only on the
verify-mode-installed path makes the flag track the real handshake. This mirrors the sibling
EdgionTls path, which tls-F001 already changed to `is_mtls = et.is_mtls_enabled()` (Terminate
mode installs no verify mode → not flagged). Both mTLS paths in this file now key `is_mtls`
off whether a verify mode was installed, not off an `Ok` return.

## Fix suggestions not accepted

- "Reject `frontendValidation` with no CA refs at the controller instead" (issue Option 2) —
  was considered; Option 1 (runtime robustness) was chosen so the data plane is correct even
  if a config slips through, without expanding the controller validator's scope.

## Re-evaluation triggers

- A new caller of `configure_gateway_frontend_mtls` ignores the returned bool.
- The controller starts fencing empty-CA-refs frontendValidation (would make the runtime path
  unreachable, but the bool return stays correct defense-in-depth).

## Reference cases

- tls-F001 (`terminate-mtls-ca-ordering.md`) — sibling EdgionTls path; same
  "key the flag/behavior off the real verify mode, not an Ok return" class.
