---
name: review-hmac-auth
description: Review knowledge for hmac-auth, grouped by decision type.
---

# hmac-auth review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Mandatory design

- [HmacAuth — cross-group duplicate usernames are first-wins](mandatory-design/dup-key-first-wins.md)
- [HmacAuth — group name appears in OK access log, not in failures](mandatory-design/group-in-ok-log.md)

## Intentional design

- [HMAC Auth Replay Protection: Timestamp Window + HTTPS + Business-Layer Idempotency is the Correct Layering](intentional-design/replay-protection-timestamp-only.md)

## Regression guards

- [HMAC Request-Body Hashing After Cheap Authentication — FIXED](regression-guards/request-body-hash-after-cheap-auth.md)
- [HMAC Body Integrity Requires a Signed-Digest Tamper Test — FIXED](regression-guards/signed-digest-body-tamper-negative-test.md)
- [HMAC Auth Wire Format Is Edgion's Contract — FIXED](regression-guards/wire-format-is-edgion-contract.md)

## Review evidence

- [HMAC Auth Signature Only Covers Declared Headers — NOT-AN-ISSUE](review-evidence/signed-headers-scope.md)
