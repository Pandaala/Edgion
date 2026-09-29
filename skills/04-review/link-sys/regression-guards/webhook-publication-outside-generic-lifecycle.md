---
name: webhook-publication-outside-generic-lifecycle
description: Use when reviewing Webhook preparation, unified runtime publication, and retirement.
knowledge_type: regression-guards
---

# Webhook preparation precedes unified runtime publication

## Current boundary

Webhook does not use the persistent connection runner. Its owned
WebhookLinkClient prepares the canonical ManagedOutboundClient before a same-type
replacement becomes visible in the unified LinkSys runtime table. Equal
fingerprints reuse the current object; failed ordinary preparation preserves an
existing same-type client. A forced rebuild after global outbound TLS changes
retires the previous Webhook publication before readiness/preparation: preserving
it would continue using an identity or trust policy that may have been revoked.
An unbuildable replacement stays absent until a later usable generation arrives.
Typed lookup cannot discover a stale independent Webhook
registration after a type change.

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
