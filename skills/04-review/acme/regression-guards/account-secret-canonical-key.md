---
name: account-secret-canonical-key
description: Use when reviewing ACME account Secret restoration and key names.
knowledge_type: regression-guards
---

# ACME account restoration uses one Secret key

## Conclusion and scope

The ACME account writer, resolver, and restoration path use
`ACME_ACCOUNT_KEY` (`accountCredentials`). A Secret containing only the old
`credentials` key has no restorable account material.

## Rationale and evidence

`services/acme/secret_io.rs::extract_account_json_string` previously read the
old key as a fallback while the writer and shared resolver used only the
canonical key. The fallback was removed; the extraction test checks the
old-key-only case. This is a pre-release input-contract cleanup, not a claim
about live certificate migration.

## Verification and re-evaluation

Re-evaluate if the ACME account Secret contract changes. The extraction unit
tests cover canonical and missing-key behavior.

## Resolution references

- `review-results/controller-gateway-overengineering-20260924-031121/ISSUE-002`.
