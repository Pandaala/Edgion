---
name: link-sys-hot-reload-reuse-and-retirement
description: Use when reviewing LinkSys Secret hot-reload documentation, client reuse, handover, or queue-drain claims.
knowledge_type: regression-guards
finding_id: ISSUE-009
---

# LinkSys hot reload reuses equal material and does not promise queue drain

## Conclusion and scope

A referenced Secret update causes the Controller to reprocess LinkSys and send
resolved configuration to the Gateway. For Redis, Etcd, Elasticsearch, Kafka,
and OTLP, an ordinary update whose consumed runtime material is unchanged
reuses the published client generation. A material change starts a replacement:
the runner retires a generation with no live connections before attempting a
new one, or keeps a generation with live connections until a new one initializes
and publishes successfully, then retires the old one. Webhook has a separate ordered
publication path and skips preparation for an equal runtime fingerprint. Delete
unpublishes and shuts down the client. Neither retirement nor deletion promises
delivery of queued Kafka or Elasticsearch records.

## Rationale and evidence

`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/link_sys.rs`
resolves referenced Secrets into the LinkSys spec. The Gateway's
`runtime/apply.rs::plan_apply` compares provider-defined runtime and consumed
Secret fingerprints, returning `Reuse` for an equal ordinary update and
`ReplaceNow` or `Handover` according to `ConnectionCoverage.live` otherwise.
`runtime/runner.rs` executes those plans: handover publishes the new generation
only after successful initialization, then schedules retirement; a failed
build or initialization need not publish a replacement. Delete unpublishes and
shuts down. The Webhook
store and manager perform their own ordered publication. Kafka shutdown aborts
and joins producer tasks; it does not flush their queued records. The accepted
runtime lifecycle specification in
`tasks/todo/linksys-runtime-apply-lifecycle/00-spec.md` explicitly excludes a
queue-drain guarantee.

ISSUE-009 found that `skills/02-features/03-resources/18-link-sys.md` and both
connector FAQ translations described every referenced Secret update as a
client rebuild with graceful draining. Those descriptions have been corrected
to distinguish reuse, replacement, handover, and the delivery limit.

## Alternatives and limits

An explicit `rebuild_clients()` request bypasses equal-fingerprint reuse; see
[forced rebuild](forced-rebuild-survives-ordinary-replay.md). The comparison is
based on consumed runtime material, so a Secret metadata-only update need not
create a new client. Handover's bounded old-generation retirement is not a
queue drain or a delivery acknowledgement. Webhook does not use the persistent
runner; see [Webhook publication](webhook-publication-outside-generic-lifecycle.md).
This page corrects documentation, not runtime behavior.

## Verification and re-evaluation

The current `apply.rs::equal_fingerprint_reuses` unit test checks the reuse
plan; the runner's `equal_fingerprint_reuses_published_arc` checks client
identity. The Kafka shutdown fence is covered separately by
[producer generation shutdown](kafka-producer-generation-shutdown.md). These
tests do not establish live backend behavior or guaranteed delivery. Re-evaluate
this description if fingerprint classification, runner planning, Webhook
publication, or producer shutdown semantics change.

## Resolution references

- `ISSUE-009` — corrected the LinkSys feature reference and connector FAQ
  descriptions of hot reload; no runtime change.
