---
name: signed-digest-body-tamper-negative-test
description: Use when reviewing HMAC request-body integration coverage; requires a valid signed Digest paired with different transmitted bytes.
status: fixed
finding_id: BODY-012
closed: 2026-07-27
---

# HMAC Body Integrity Requires a Signed-Digest Tamper Test — FIXED

## Conclusion

HMAC request-body integration coverage must include a request whose
Authorization and Digest headers are valid for one body while different bytes
are transmitted. A matching-body success case alone does not prove that the
runtime validates body integrity.

## Core Rationale

**1. Header authentication and body integrity are separate checks**

The Authorization signature authenticates the Digest header value. The runtime
must additionally hash the current request-body version and compare it with
that signed value when `validateRequestBody` is enabled.

**2. The negative case isolates body validation**

The test signs `original-body`, preserves the resulting Date, Digest, and
Authorization headers, then sends `tampered-body`. Header-level authentication
remains valid, so the expected `401` can only be explained by the body-digest
mismatch.

**3. Public status is the stable assertion**

The test checks the documented authentication status without matching internal
error text. Diagnostic wording may change without weakening fail-close body
integrity.

## Fix Suggestions Not Accepted

- Keep only the matching-body `200` case — an implementation that skips body
  validation could still pass.
- Corrupt the signature or Digest header — that tests header authentication,
  not comparison with transmitted bytes.
- Assert an internal error string — the public status is the stable contract.

## Re-evaluation Triggers

Re-open this decision only if:

- HMAC request-body validation is intentionally removed.
- The documented public rejection status changes.
- The request-body mutation ordering changes which committed body version HMAC
  must validate.

## Reference Cases

- `BODY-012`
- `edgion-tests/src/client/suites/edgion_plugins/body_plugins.rs`
- `skills/04-review/hmac-auth/request-body-hash-after-cheap-auth.md`
