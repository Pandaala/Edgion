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

## Account registration log privacy (ARCH-141)

`AcmeClient::new` and `new_with_ca` send the configured `mailto:` contact to the
CA but omit the email from their successful-registration events. The events
retain the server, account identifier and outcome. This follows the email PII
rule in [log safety](../../../03-coding/01-log-safety.md); account protocol and
Secret persistence/restoration behavior are unchanged.

Verification of this narrow logging correction is inspection of both event field
sets and unchanged contact construction. Existing account and service tests cover
their own lifecycle contracts, not emitted registration-event privacy. Re-evaluate
if registration events gain contact data or new account-creation paths are added.
