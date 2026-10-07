---
name: webhook-publication-outside-generic-lifecycle
description: Use when reviewing Webhook preparation, unified runtime publication, and retirement.
knowledge_type: regression-guards
---

# Webhook preparation precedes unified runtime publication

## Current boundary

Webhook does not use the persistent connection runner. Its owned
WebhookLinkClient prepares the canonical ManagedOutboundClient before a same-type
replacement becomes visible in the unified LinkSys runtime table. WebhookCandidate
retains the actual LinkSys Arc, reference-only topology and one captured global
TLS snapshot. Incoming source, full active local reference/material pairing and
inherited authority precede both equal-fingerprint and same-RV reuse. Absence or
disabled local TLS still proves the source, then keeps native system trust and no
global client identity. Failed ordinary preparation preserves an
existing same-type client. A forced rebuild after global outbound TLS changes
retires the previous Webhook publication before readiness/preparation: preserving
it would continue using an identity or trust policy that may have been revoked.
An unbuildable replacement stays absent until a later usable generation arrives.
Typed lookup cannot discover a stale independent Webhook
registration after a type change.

Ordinary custom LinkSys retention keeps the previous current publication available
to subsequent new call lookups, including when the denied replacement has lost
Grant-backed template/TLS material. It is not limited to previously borrowed
handles. This existing lifecycle contract differs from forced withdrawal and from
standard Gateway API Grant-required fields, whose denied publication must stop
new target/material use. Source-bound proof blocks invalid incoming reuse/build;
it does not introduce immediate revocation of a retained custom publication.

ManagedOutboundClient::from_webhook accepts the actual bound LinkSys and extracts
only its fixed Webhook variant internally. Its private construction core retains
the canonical target/DNS/egress/template/retry/health behavior and delegates the
proved HTTP factory with the same captured snapshot. No detached Webhook config
constructor or raw TLS fallback remains. Service targets and their health probes
derive omitted namespace from the validated source; omitted owner namespace means
default, while explicitly invalid source metadata fails even without active TLS.
Fingerprint preparation serializes a borrowed redacted connection projection,
hashes actual template/TLS material separately, and shares the masked inherited
encoder with HTTPDNS. Comparison performs no PEM parsing or TLS audit.
Complete custom Service identity and EP/Route transport migration remain open in
tasks/pending/resource-reference-contract/; this is a scoped Webhook boundary.

Retirement explicitly stops the managed health task without waiting for the last
external call handle. Caller-held handles are not forcibly revoked. See
[provider-owned removal](../intentional-design/provider-owned-runtime-and-simple-removal.md).

## Historical mechanism

The earlier implementation used a sequence-checked WebhookManager mutation guard:
prepare, commit the services map, then update WEBHOOK_RUNTIME bookkeeping.
The unused LinkSysLifecycle trait could not provide that transaction. That was
the basis of link-sys-lifecycle-webhook-doc (2026-09-12).

The unified-table refactor removes that trait, the services map, registration
identity, mutation guard, and duplicate bookkeeping. Ordered source dispatch now
owns ordering for all providers; the old manager-first/bookkeeping-second rule
is no longer an implementation requirement. Its useful invariant survives:
never publish an incompletely prepared client.

## Evidence and re-evaluation

Owning sources are link_sys/runtime/store.rs, providers/webhook/client.rs,
providers/webhook/manager.rs, and the canonical outbound engine under
plugins/http/common/outbound/. Review source-event ordering, same-type failure
preservation, forced TLS invalidation, recovery, kind changes, and full-set omission
whenever publication changes. The store test
`webhook_forced_tls_rebuild_revokes_unusable_generation_and_recovers` covers both
readiness and preparation failures without changing ordinary last-good behavior.
The [provider-owned removal page](../intentional-design/provider-owned-runtime-and-simple-removal.md)
records the unified table's verification boundary; historical verification of
the removed manager does not verify the new table.

## Resolution references

- link-sys-lifecycle-webhook-doc: historical documentation correction.
- link-sys-cross-provider-partial-replacement: unified client authority removes
  the independent stale Webhook registration.
