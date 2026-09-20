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
fingerprints reuse the current object; failed preparation preserves an existing
same-type client. Typed lookup cannot discover a stale independent Webhook
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
preservation, kind changes, and full-set omission whenever publication changes.
The active refactor task records fresh test results; historical verification of
the removed manager does not verify the new table.

## Resolution references

- link-sys-lifecycle-webhook-doc: historical documentation correction.
- link-sys-cross-provider-partial-replacement: unified client authority removes
  the independent stale Webhook registration.
