---
name: shared-tls-fingerprint-encoding
description: Use when reviewing LinkSys provider TLS policy and Secret fingerprint encoding.
knowledge_type: regression-guards
---

# LinkSys providers share TLS fingerprint encoding

## Conclusion and scope

Five persistent providers use one canonical TLS policy encoder. Those
providers, Webhook, and prepared HTTPDNS clients use one TLS material encoder
in Common's `link_sys_core/fingerprint.rs`. Gateway re-exports that helper;
Controller HTTPDNS preparation consumes the same implementation through Common.
Provider-specific configuration and credential hashing remain local. Webhook
filters disabled TLS before hashing its prepared connection. HTTPDNS hashes its
redacted configuration plus enabled TLS material; disabled TLS policy fields can
still change its configuration identity, while disabled Secret bytes do not.

## Rationale and evidence

The provider-local encoders previously repeated the same option tags, ordered
SAN/reference/CA lists, and sorted Secret data-key encoding. A shared helper
now lives in `edgion-common/src/link_sys_core/fingerprint.rs`, allowing both
Controller and Gateway to reuse it without another encoder. Secret metadata
remains excluded. HTTPDNS material identity is separate from its redacted
address-cache fingerprint; see [prepared HTTPDNS runtime](httpdns-prepared-runtime.md).

## Verification and re-evaluation

Shared helper tests cover policy variants, enabled/disabled material, changed
Secret bytes, and metadata exclusion. Re-evaluate if a provider begins consuming
a different subset of TLS material or changes its rebuild policy.

## Resolution references

- `review-results/controller-gateway-overengineering-20260924-031121/ISSUE-009`.
  This is distinct from the earlier, unrelated `ISSUE-009` in the LinkSys hot
  reload documentation review.
