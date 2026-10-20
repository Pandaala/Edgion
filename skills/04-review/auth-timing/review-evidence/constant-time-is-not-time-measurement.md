---
name: constant-time-is-not-time-measurement
description: Use when a finding proposes adding constant-time comparison (subtle / ct_eq) to time/duration code such as access-log relative timestamps, request-latency, or duration metrics; explains that constant-time comparison applies only to secret equality checks, not to wall-clock measurement, and records a historical 2026-06 audit that requires current revalidation.
knowledge_type: review-evidence
---

# "Constant-Time" Is Secret-Equality, Not Time Measurement — NOT-AN-ISSUE

## Conclusion

A constant-time comparison (`subtle::ConstantTimeEq` / `ring::constant_time` / a
hand-rolled XOR-fold) is **only** relevant when an attacker-supplied value is
compared for equality against a **server-side secret**, where the comparison's
data-dependent timing could leak the secret byte-by-byte. It has **nothing** to
do with wall-clock time measurement.

Fix suggestions of the form "the access log records relative time / request
latency / durations — should those be reworked with `subtle`?" or "harden the
duration/timing code to be constant-time" are **not accepted**. Access-log
relative timestamps, request-latency fields, and the request-duration histogram
are *measurements* of how long work took; there is no secret and no equality
comparison, so constant-time techniques are inapplicable and meaningless there.
The name "constant-**time**" refers to *comparison cost being independent of the
data*, not to processing time.

## Core Rationale

**1. Two unrelated concepts share the word "time".**

- Constant-time **comparison**: removes the data-dependent early-return when
  checking `presented == secret`, so an attacker cannot use response-time
  differences to recover the secret one byte at a time. Operates on byte
  sequences that are secrets.
- Time **measurement** (access-log `Duration`, latency, `request_duration`
  histogram, relative timestamps): the value *is the thing being recorded*.
  There is no second value to compare against, no secret, and no equality gate.
  `subtle::ConstantTimeEq` does not even apply to `Duration`/`Instant` arithmetic.

**2. Historical whole-surface audit (2026-06).**

The following table records earlier implementation observations. It is not a
fresh whole-surface verification or a reason to suppress current contrary
evidence. Recheck the code, dependencies and assumptions before using a row:

| path | timing-safe primitive |
|------|----------------------|
| `gateway/api/admin_auth.rs` (admin bearer token) | `subtle::ConstantTimeEq` on SHA-256 digests |
| `plugins/http/key_auth` | SHA-256 + `HashMap` lookup (see [key-auth-hashmap-not-ct.md](key-auth-hashmap-not-ct.md)) |
| `plugins/http/hmac_auth` | `hmac::Mac::verify_slice` |
| `plugins/http/csrf` (token sig) | Full source-level XOR fold after lifetime and public-length checks; supplied nonce is not secret (see [timing rules](../../security/mandatory-design/iron-rules.md)) |
| `plugins/http/basic_auth` (htpasswd) | constant-time XOR-fold (SHA-1 / MD5-crypt / APR1) |
| `controller/authz` (admin/cli token) | SHA-256 (`sha256_hex`) + `HashMap` lookup (token-only admin auth; same class as `key_auth`) |
| `jwt_auth` / `openid_connect` (gateway plugins) | `jsonwebtoken::decode` (lib-side constant-time MAC/sig) |
| `header_cert_auth` | X.509 asymmetric `verify_signature` |

The only site this project added `subtle` to is `admin_auth.rs`, and that was
justified by its opt-in non-loopback exposure tier. This history does not establish that every current comparison
is safe or needs no further review.

**3. Industry precedent.** Envoy / Istio / Kong likewise apply constant-time
compare only to credential/MAC verification, never to latency or access-log
timing fields. Access-log durations are emitted verbatim everywhere.

## Fix Suggestions Not Accepted

- "Rework access-log relative-time / latency / `request_duration` with `subtle`"
  — category error; no secret, no comparison; `ConstantTimeEq` does not apply to
  duration arithmetic.
- "Add constant-time comparison to `key_auth` / `hmac_auth` / `csrf` / JWT paths"
  — do not infer a defect solely from the absence of a particular library API.
  Verify the current primitive and threat model using the linked evidence.
  CSRF nonce visibility does not eliminate the need to assess its comparison;
  a full source-level fold is not by itself a machine-code timing proof.

## Re-evaluation Triggers

Re-evaluate when:

- An existing implementation, dependency, compiler behavior or threat-model
  assumption changes, or current evidence contradicts a historical conclusion.
- A new auth path is added that compares an attacker-supplied value against a
  server-side secret using a naive `==` / `eq` (then it needs `ConstantTimeEq`
  or `verify_slice`, judged against the iron-rules entropy/jitter test).
- A secret value is ever written into a timing/measurement field (it should not
  be — that would be a redaction bug, not a constant-time one).

## Reference Cases

- 2026-06 gateway admin-auth feature + whole-surface secret-comparison audit.
- `skills/04-review/security/mandatory-design/iron-rules.md` — project-wide timing-attack test
  (entropy + network-jitter signal-to-noise) to apply before flagging any
  "non-constant-time comparison" finding.
- `skills/04-review/admin-api/` — the one site that legitimately uses
  `subtle::ConstantTimeEq`.
