---
name: mtls-leaf-only-whitelist-verification
description: BoringSSL mTLS allowlist runs only at leaf depth 0 and emits at most one terminal ssl.log. Reject re-reports that the callback re-evaluates SAN/CN at every chain depth.
status: fixed
finding_id: mtls-leaf-only-whitelist-verification
closed: 2026-09-05
---

# mtls-leaf-only-whitelist-verification — leaf-only SAN/CN allowlist

## Conclusion

The BoringSSL `SSL_set_verify` callback in
`edgion-gateway/src/tls/boringssl/mtls_verify_callback.rs` now classifies each
invocation by `preverify_ok` and `X509_STORE_CTX_get_error_depth`. Chain
failures reject at every depth. Client-identity extraction and SAN/CN allowlist
evaluation run only at leaf depth 0. Intermediate successful depths return 1
without extraction or a whitelist audit. One `VerifyContext.audit_emitted` flag
owns the terminal `ssl.log` record for that handshake.

Re-reports of the form "the verify callback re-runs the leaf allowlist at every
certificate-chain depth", "intermediate callbacks extract `peer_certificate()`",
or "a multi-cert chain duplicates the same whitelist failure log" are **not
accepted**.

## Core Rationale

**1. BoringSSL calls the callback per depth, not once per handshake.**
`SSL_set_verify` is invoked for each certificate in the presented chain.
`ssl.peer_certificate()` is always the leaf, so repeating the allowlist did not
change accept/reject, but it repeated handshake work and could emit the same
whitelist audit more than once.

**2. Leaf depth is the only identity owner.**
`classify_verify_callback` is the testable seam: `preverify_ok == 0` →
`RejectChain`; `depth > 0` → `AcceptIntermediate`; `depth == 0` with context →
`EvaluateLeafAllowlist`; missing leaf context or a negative depth stays
fail-closed. SAN/CN comparison semantics are unchanged.

**3. Exactly-once audit is handshake-scoped, not per callback.**
`claim_terminal_audit` on `VerifyContext.audit_emitted` lets a chain-verify
failure, missing leaf context, or allowlist mismatch emit at most one `ssl.log`
when context is present. The panic hook uses the same emitter. Returning 0 from
a `RejectChain` / fail-close arm stops further BoringSSL verify-callback
invocations, so a missing `VerifyContext` still has a single writer for that
handshake. A second in-callback `ssl.log` writer that does not consult
`audit_emitted` (and does not return 0) would be a regression.

## Fix Suggestions Not Accepted

- Re-checking the allowlist at every successful depth "for defense in depth" —
  the leaf is the client identity; intermediates are already covered by
  `preverify_ok`.
- Logging a whitelist result from intermediate depths.
- Changing SAN/CN match semantics, verify-depth policy, or adding rustls
  Gateway parity.
- Extracting `X509_STORE_CTX_get_current_cert` at intermediate depths and
  comparing those identities to the allowlist.

## Re-evaluation Triggers

- BoringSSL stops invoking the callback at depth 0 for a presented client
  certificate.
- A second mTLS allowlist path is added that does not go through this
  classifier.
- The handshake gains a second `ssl.log` writer on the verify-callback reject
  path without consulting `audit_emitted`.

## Reference Cases

- `tasks/todo/mtls-leaf-only-whitelist-verification.md` (closed by this fix)
- `edgion-gateway/src/tls/boringssl/mtls_verify_callback.rs`
- Unit tests in that file: one-through-nine-cert walks, preverify rejection,
  missing leaf context, and single terminal-audit claim
