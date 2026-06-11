---
name: terminate-mtls-ca-ordering
description: configure_mtls now decides verify mode and returns early for Terminate before any CA loading. Reject re-reports claiming Terminate without caSecretRef aborts the handshake.
---

# tls-F001 — `configure_mtls` returns before CA loading for `Terminate` mode

**Status:** ✅ fixed · 2026-05-20
**Severity:** Critical (BadLogic) — a valid config caused a per-SNI TLS outage

## Conclusion

`configure_mtls` (`src/core/gateway/tls/runtime/gateway/tls_pingora.rs`) now evaluates the
`client_auth.mode` match **first** and returns `Ok(())` for `ClientAuthMode::Terminate`
before calling `edgion_tls.ca_cert_pem()` or installing any `X509Store`. CA loading runs only
on the `Mutual` / `OptionalMutual` arms.

Re-reports of the form "Terminate mode with no `caSecretRef` aborts every handshake",
"`configure_mtls` reads the CA secret unconditionally", or "redundant CA store installed for
Terminate (F-006)" are **not accepted** — both are no longer true.

## What was wrong (before the fix)

`configure_mtls` opened by unconditionally calling `ca_cert_pem()`, then loaded and installed
the client CA store, and only reached the `client_auth.mode` branch afterwards (the
`Terminate => return Ok(())` arm was the *last* statement before the verify-mode block).

`ca_cert_pem()` (`edgion-resources/src/resources/edgion_tls.rs`) returns
`Err("CA secret not loaded by controller")` when `client_auth.ca_secret` is `None`. The
controller only populates `ca_secret` for resources that carry a `ca_secret_ref`. The
validator (`validate_mtls_config`, `src/core/gateway/tls/validation/cert.rs`) explicitly
allows `Terminate` mode **without** a `caSecretRef`. So a valid Terminate-only EdgionTls with
no `caSecretRef` passed admission, then failed at runtime: the `?` on `ca_cert_pem()`
propagated `Err` out of `configure_mtls` → `apply_edgion_tls_cert` set
`ssl_ctx.meta.err_log = Some("mtls config: ...")` and aborted **every** handshake for that SNI.

## Why the fix is correct

Terminate mode performs TLS termination only — it never requests or verifies a client
certificate, so it needs no client CA store. Returning before CA loading:

1. Makes a valid no-`caSecretRef` Terminate config work (the bug fix).
2. Skips the redundant CA store install for Terminate-with-`caSecretRef` (subsumes F-006); a
   store set without a verify mode is never consulted, so there is no observable change there.
3. Keeps the `is_mtls` flag correct — `select_cb` already computes it via `is_mtls_enabled()`
   (which excludes Terminate), so only the cert_cb path was broken, never the flag.

The `Mutual` / `OptionalMutual` arms are byte-for-byte unchanged; the only edit reorders the
`verify_mode` match above the CA-loading block.

## Regression guard

`examples/test/conf/EdgionTls/mTLS/EdgionTls_edge_mtls-test-terminate.yaml` previously carried
a `caSecretRef` solely to work around this bug (a comment said so). The ref was removed so the
existing Terminate suite cases — `test_terminate_without_cert` and `test_terminate_with_cert`
in `examples/code/client/suites/edgion_tls/mtls/mtls.rs` — now exercise a genuine no-CA
Terminate listener. Before the fix, removing that ref would make both cases fail at the
handshake.

## Re-evaluation triggers

- A new `ClientAuthMode` variant is added — re-check that its CA-loading needs are routed
  through the leading match, not assumed by position.
- The validator starts requiring `caSecretRef` for `Terminate` — then the early return and the
  no-CA test config must change together.

## Reference

- Issue: `tasks/issues/tls-F001-configure-mtls-aborts-terminate-no-ca.md` (Round B: confirmed)
- Doc (already correct pre-fix): `docs/en/ops-guide/gateway/tls/edgion-tls.md` — `caSecretRef`
  marked Conditional, "required when mode is Mutual or OptionalMutual"
