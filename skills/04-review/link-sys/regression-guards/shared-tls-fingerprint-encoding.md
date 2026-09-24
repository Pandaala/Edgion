---
name: shared-tls-fingerprint-encoding
description: Use when reviewing LinkSys provider TLS policy and Secret fingerprint encoding.
knowledge_type: regression-guards
---

# LinkSys providers share TLS fingerprint encoding

## Conclusion and scope

Five persistent providers use one Gateway LinkSys TLS policy encoder. Those
providers and Webhook use one TLS material encoder. Provider-specific
configuration and credential hashing remain local; Webhook still hashes its
prepared connection through serialization after filtering disabled TLS.

## Rationale and evidence

The provider-local encoders previously repeated the same option tags, ordered
SAN/reference/CA lists, and sorted Secret data-key encoding. A shared helper
in `link_sys/runtime/apply.rs` removes the drift opportunity without changing
the position of each hash contribution. Secret metadata remains excluded.

## Verification and re-evaluation

Shared helper tests cover policy variants, enabled/disabled material, changed
Secret bytes, and metadata exclusion. Re-evaluate if a provider begins consuming
a different subset of TLS material or changes its rebuild policy.

## Resolution references

- `review-results/controller-gateway-overengineering-20260924-031121/ISSUE-009`.
  This is distinct from the earlier, unrelated `ISSUE-009` in the LinkSys hot
  reload documentation review.
