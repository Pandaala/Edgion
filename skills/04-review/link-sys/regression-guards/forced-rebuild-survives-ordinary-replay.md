---
name: forced-rebuild-survives-ordinary-replay
description: Use when reviewing LinkSys forced rebuilds interleaved with ordinary desired-state posts.
knowledge_type: regression-guards
finding_id: ISSUE-004
---

# A LinkSys forced rebuild survives ordinary replay

## Conclusion and scope

For Redis, Etcd, Elasticsearch, Kafka, and OTLP persistent runners, a buildable
`LinkSysStore::rebuild_clients()` request must bypass fingerprint reuse until a
new client generation successfully publishes. An equal ordinary resource replay
or temporary `NotReady` state cannot clear that pending request. This rule covers
the per-key `PersistentRunner`; Webhook has a separate publication path.

## Rationale and evidence

The LinkSys lifecycle specification requires forced rebuilds because clients
inherit process-wide settings at construction
(`tasks/todo/linksys-runtime-apply-lifecycle/00-spec.md`, Forced rebuild).
`LinkSysStore::rebuild_clients` dispatches the current source snapshot with
`forced=true`, and each persistent provider passes that flag to
`PersistentRunner::post_ready`. The runner's `Desired.force_gen` records the
latest forced post generation. Ordinary `Ready` posts advance `Desired.gen` but
preserve `force_gen`; the loop compares it with its last successfully fulfilled
force generation before calling `plan_apply`. A newer desired generation still
invalidates an unpublished candidate, and only successful publication advances
the fulfilled force generation.

ISSUE-004 identified the previous gap: `post_ready` stored `forced` only on the
latest `DesiredState::Ready`. An equal unforced replay overwrote `true`, cancelled
the in-flight candidate through `gen`, then allowed `plan_apply` to reuse the
old client's equal fingerprint. The same overwrite could occur before the loop
polled either post.

## Alternatives and limits

Treating `forced` as a property of the latest resource post loses an outstanding
process-wide refresh request. Keeping the request until successful publication
also covers failed build/init and temporary `NotReady` without changing the
provider fingerprint or the published client lookup. A real delete ends the
runner and its pending request. This guard does not cover the separate Webhook
preparation path. The former pending-recreation delete joiners have been removed;
new objects admitted after capacity recovery use the current process-wide settings.

## Verification and re-evaluation

`cargo test -p edgion-gateway --lib equal_replay_` passed both focused tests on
2026-09-17. One test coalesces the forced post with an equal ordinary replay
before the runner polls; the other injects that replay just before candidate
publication. These fake-owner tests verify runner publication and Arc replacement,
not live external connections or every scheduler order. Re-evaluate if desired
posts, generation checks, successful publication accounting, or provider dispatch
ordering changes.

## Resolution references

- `ISSUE-004` — corrected loss of a pending forced rebuild on 2026-09-17.
