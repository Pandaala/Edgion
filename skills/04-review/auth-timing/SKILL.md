---
name: review-auth-timing
description: Review knowledge for auth-timing, grouped by decision type.
---

# auth-timing review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Mandatory design

- [HTTP auth plugins — reuse `auth_common::apply_auth_failure_delay`](mandatory-design/shared-apply-auth-failure-delay.md)

## Intentional design

- [Basic Auth: bcrypt is not supported (by design)](intentional-design/basic-auth-no-bcrypt.md)
- [LDAP Auth Cache Timing — Resolved by Cache Removal](intentional-design/ldap-auth-cache-timing-oracle.md)

## Regression guards

- [Basic Auth Username-Existence Timing Difference: No Code-Layer Dummy Verify](regression-guards/basic-auth-username-enumeration.md)
- [HeaderCertAuth Failure Delay Uses the Shared Helper — FIXED](regression-guards/header-cert-auth-shared-failure-delay.md)

## Review evidence

- ["Constant-Time" Is Secret-Equality, Not Time Measurement — NOT-AN-ISSUE](review-evidence/constant-time-is-not-time-measurement.md)
- [Key Auth HashMap Lookup is Not Constant-Time: Not a Real Attack Surface](review-evidence/key-auth-hashmap-not-ct.md)
