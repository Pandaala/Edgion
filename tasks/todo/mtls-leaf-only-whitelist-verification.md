# Run mTLS whitelist verification once for the leaf certificate

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | pending |
| Type | bugfix |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Make the BoringSSL client-certificate verify callback run the Edgion SAN/CN allowlist only for the verified leaf certificate. BoringSSL invokes the callback at each certificate-chain depth, while the current callback repeatedly extracts the peer leaf certificate and reruns the same allowlist work at every successful depth.

## Requirements

- Check and preserve `preverify_ok` failure handling at every certificate-chain depth.
- Run client identity extraction and SAN/CN allowlist validation only when `X509_STORE_CTX_get_error_depth` reports leaf depth zero.
- Preserve fail-close behavior for missing callback state, missing certificates, invalid chains, and allowlist mismatches.
- Preserve panic isolation with `edgion_common::startup::catch_redacted_panic`.
- Define one terminal TLS audit owner so a multi-certificate chain does not duplicate the same whitelist failure log.

## Scope

### In scope

- `edgion-gateway/src/tls/boringssl/mtls_verify_callback.rs`.
- A callback test seam for chain depth, extraction count, rejection, and log ownership.
- Downstream TLS architecture documentation if callback ordering changes.

### Out of scope

- Changes to SAN/CN comparison semantics.
- Certificate-chain construction or verification-depth policy.
- Gateway rustls backend support or parity.

## Acceptance criteria

- Successful chains from one through nine certificates perform exactly one leaf identity extraction and allowlist evaluation.
- A `preverify_ok == 0` result at any depth immediately rejects the handshake.
- Missing context at leaf depth remains fail-close.
- Intermediate successful callbacks neither extract the leaf certificate nor emit a whitelist result.
- Every terminal failure path emits at most one `ssl.log` record.
- Targeted callback tests, workspace check, Clippy, and repository guards pass.

## Related guidance

- `skills/00-design-rules.md`
- `skills/01-architecture/02-gateway/04-tls/SKILL.md`
- `skills/01-architecture/02-gateway/04-tls/01-downstream-tls.md`
- `skills/03-coding/01-log-safety.md`
- `skills/05-testing/SKILL.md`

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-31 | Defer the callback change from the validation-file batch. | The fix crosses BoringSSL per-depth callback ordering, certificate extraction, and exactly-once TLS audit behavior and needs a dedicated test seam rather than an unverified FFI branch. |
