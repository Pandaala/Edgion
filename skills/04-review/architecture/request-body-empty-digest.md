---
name: request-body-empty-digest
description: Use when reviewing body-aware hashing of requests with no declared body; covers why an empty body is a valid version without requiring capture.
status: fixed
finding_id: BODY-007
closed: 2026-07-27
---

# BODY-007 — Empty Request-Body Digest — FIXED

## Conclusion

A request that admission proves has no body hashes as the empty byte sequence.
It does not need a synthetic downstream read, a `BodyStorage` allocation, or a
temporary file merely to calculate its SHA-256 digest.

Requests that declare a body still require complete bounded capture and hash
through `BodyStorageView`. Capture errors must never fall back to the empty
digest.

## Core Rationale

**1. Bodyless admission is authoritative**

`RequestAdmission::PassThrough` requires the immutable request-entry snapshot
to say that no body was declared. Its successful result is therefore the
canonical empty V0, not an incomplete capture.

**2. Capture failures remain loud**

The `Buffer` path installs a view only after capture completes. Limit,
timeout, and incomplete-read failures return their original error before the
empty-body fallback can run.

**3. The fallback composes with later bodyless mutation support**

If a mutating plugin later installs a real empty V0 view for a bodyless
request, hashing automatically uses that view. Any committed non-empty version
is then visible to later plugins without changing the digest API.

## Fix Suggestions Not Accepted

- Read from the downstream transport to prove an already-admitted bodyless
  request is empty.
- Allocate disk-backed storage solely to hash empty bytes.
- Treat any missing body view as empty without first running request admission.
- Convert capture failures for declared bodies into the empty digest.

## Re-evaluation Triggers

Re-open this decision only if:

- `PassThrough` can succeed for a request whose body declaration is unknown or
  positive.
- Request-body hashing moves before the admission decision.
- Body versions can be published outside the canonical request-body owner.

## Reference Cases

- Origin finding:
  `tasks/review-body-buffering-runtime/body-007-hmac-empty-body-digest.md`
- Admission and digest adapter:
  `edgion-gateway/src/plugins/runtime/session_adapter.rs`
- HMAC verification:
  `edgion-gateway/src/plugins/http/hmac_auth/plugin.rs`
