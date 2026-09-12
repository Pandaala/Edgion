---
name: webhook-publication-outside-generic-lifecycle
description: Use when reviewing LinkSysLifecycle dispatch or Gateway Webhook publication and replacement.
knowledge_type: regression-guards
---

# Webhook Publication Stays Outside the Generic LinkSys Lifecycle

## Conclusion and scope

`LinkSysLifecycle` is the shared contract for providers using the generic
LinkSys dispatch helpers. Redis, Etcd, Elasticsearch, and Kafka currently use
it. `WebhookLinkClient` deliberately does not implement the trait; Gateway
Webhook publication is owned by `WebhookManager` and its ordered mutation
guard.

## Rationale and evidence

`dispatch_full_set_with` and `dispatch_partial_update_with` acquire a Webhook
mutation guard with the incoming sequence, prepare each candidate, commit the
manager mutation, and only then update `WEBHOOK_RUNTIME`. This preserves stale
sequence rejection and prevents runtime bookkeeping from preceding publication.
The generic lifecycle helpers instead initialize and shut down providers around
runtime-store replacement and do not provide that publication transaction.

The architecture contract is documented in
`skills/01-architecture/02-gateway/09-link-sys.md`; the owning implementations
are `edgion-gateway/src/link_sys/runtime/store.rs` and
`edgion-gateway/src/link_sys/providers/webhook/manager.rs`.

## Alternatives and limits

`SHUTDOWN_ON_REPLACE = false` remains a valid generic trait capability for a
provider whose `init` supplies complete replacement semantics. It must not be
used to describe the current Webhook path. Moving Webhook onto the generic
lifecycle would require preserving its sequence check, prepare-before-publish,
manager commit, and bookkeeping ordering; a trait implementation alone is not
equivalent.

## Verification and re-evaluation

Re-evaluate this boundary if Webhook publication is redesigned or the generic
dispatch contract gains an ordered transactional publication mechanism. The
2026-09-12 documentation correction was checked against all current
`LinkSysLifecycle` implementations and both Webhook dispatch paths. Independent
review found no issue. `cargo fmt --all`, `cargo check --workspace --all-targets`,
`cargo clippy --workspace --all-targets`, and the agent-document, SSA-force,
metrics-inventory, and Pingora-source guards passed; Clippy reported only
pre-existing warnings outside this documentation-only change.

## Resolution references

- `link-sys-lifecycle-webhook-doc`, documentation finding fixed on 2026-09-12.
