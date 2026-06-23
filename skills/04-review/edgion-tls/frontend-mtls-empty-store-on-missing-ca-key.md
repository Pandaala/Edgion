---
name: frontend-mtls-empty-store-on-missing-ca-key
description: configure_gateway_frontend_mtls now returns Err when every referenced CA Secret lacks the `ca.crt` key, so a non-empty `resolved_frontend_ca_secrets` whose entries are all missing the conventional key no longer silently installs an empty trust store. Reject re-reports of the silent-empty-store form.
---

# tls-FB01 — Gateway `frontendValidation` silently installed empty CA store on missing `ca.crt`

**Status:** ✅ fixed · 2026-05-27
**Severity:** Medium (BadLogic / silent availability regression) — handshake stays fail-closed, but the listener silently rejects every client cert with no operator-visible signal

## Conclusion

`configure_gateway_frontend_mtls` (`edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs`) now
tracks `certs_added: usize` across the per-Secret loop. When every entry in a non-empty
`resolved_frontend_ca_secrets` lacks the `CA_CERT` (`ca.crt`) key, the loop body never runs,
`certs_added` stays `0`, and the function returns `Err(PingoraError::explain(InvalidCert, ...))`
instead of building an empty trust store and stamping a verify mode on top of it. The caller
(`apply_gateway_tls_cert`) already routes that `Err` into `ssl_ctx.meta.err_log` as
`"frontend mTLS error: ..."`, so the misconfiguration becomes a fail-closed listener with a
diagnostic instead of a silent client-cert barrier.

Re-reports of the form "`configure_gateway_frontend_mtls` builds an empty `X509Store` and
calls `set_verify` on it when CA Secrets lack `ca.crt`", "`add_ca_bundle_to_store`'s
fail-closed branch is bypassed by a missing key", or "no signal when frontend CA Secrets have
no usable PEM" are **not accepted** — verify the `certs_added == 0` guard is present before
re-reporting.

## What was wrong (before the fix)

The loop body only entered when both `secret.data` was `Some` **and** the data map contained
`CA_CERT`:

```rust
for secret in ca_secrets {
    if let Some(data) = &secret.data {
        if let Some(ca_bytes) = data.get(CA_CERT) {
            add_ca_bundle_to_store(&mut store_builder, &ca_bytes.0)?;
        }
    }
}
let store = store_builder.build();
ssl.set_verify_cert_store(store)?;
ssl.set_verify(SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT);
```

`add_ca_bundle_to_store` is itself fail-closed for empty/garbage PEM (returns `Err` on an
empty `X509::stack_from_pem` result), but that guard is reachable only when the key exists.
If every Secret had `data = None` or a `data` map without the `ca.crt` key (e.g. the operator
used `ca.pem`, or referenced a `kubernetes.io/tls` Secret that only carries `tls.crt`/`tls.key`),
the inner body was skipped on every iteration, `store_builder.build()` produced an empty
`X509Store`, `set_verify` was then layered on top, and the function returned `Ok(true)`.
Result: a complete client-cert barrier with no diagnostic — every TLS client was rejected at
handshake; operators learned about the failure only from client-side errors.

## Why the fix is correct

The fix restores symmetry with the existing per-Secret fail-closed branch inside
`add_ca_bundle_to_store`: "PEM present but empty/garbage" already returns `Err`. "Key absent
from every Secret" is the same class of misconfiguration (no usable trust anchors) and should
fail the same way. The configure-time `Err` populates `ssl_ctx.meta.err_log` via the existing
caller arm, so the listener fails closed with a diagnostic instead of silently denying every
handshake. Valid configurations are unaffected — `certs_added > 0` exits the guard
immediately.

Returning `Err` (over emitting `warn!` + `Ok(false)`) was preferred because:

- `ssl_ctx.meta.err_log` is the project's existing channel for surfacing TLS init failures to
  `tls.log`; the `warn!` alternative would produce a separate signal and an `Ok(false)` would
  paint the listener as "intentional no-mTLS", which is the opposite of the operator intent.
- The `Err` path is symmetric with the already-existing empty-PEM fail-closed inside
  `add_ca_bundle_to_store` (same `ErrorType::InvalidCert`), so reviewers reading the function
  see one consistent failure mode for "no usable CA bytes."
- No data-plane `tracing` is added; the check runs once per listener at SSL-callback time,
  not on the request hot path.

## Fix suggestions not accepted

- "Reject the misconfiguration at the controller validator instead" — out of scope here; the
  controller's `has_all_ca_refs_invalid` only fences zero-refs, not Secrets that contain
  unexpected keys. The runtime guard is the right defense for "config slipped through".
- "Downgrade to `warn!` + `Ok(false)` to keep mTLS optional" — would mask a real
  misconfiguration as a working no-mTLS listener; the operator declared `frontendValidation`,
  so a missing trust anchor is an error, not a "skip silently" signal.
- "Add a per-Secret `data.get(CA_CERT).ok_or(...)` inside the loop" — would mis-report when a
  later Secret carries `ca.crt` legitimately; the count-and-check-after-loop shape is the
  only one that correctly handles a multi-Secret list.

## Re-evaluation triggers

- The controller starts fencing "frontendValidation references Secrets without `ca.crt`" at
  admission time (would make the runtime guard unreachable for that exact misconfiguration,
  but the guard stays correct defense-in-depth — keep it).
- A new key name (other than `ca.crt`) becomes a supported source of frontend CA bytes — the
  guard would need to count any of the accepted keys, not only `CA_CERT`.

## Reference cases

- tls-F010 (`frontend-validation-is-mtls-empty-ca.md`) — sibling fix for the
  zero-CA-refs case (return `Ok(false)`, do not stamp `is_mtls`). This entry covers the
  adjacent case where refs exist but contain no usable PEM.
- tls-F001 (`terminate-mtls-ca-ordering.md`) — same class: key the verify-mode
  install off the real CA availability, not off the function reaching its end.
- `add_ca_bundle_to_store` empty-PEM guard (same file) — the symmetric per-Secret fail-closed.
