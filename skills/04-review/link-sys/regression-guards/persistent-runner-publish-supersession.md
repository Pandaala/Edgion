---
name: persistent-runner-publish-supersession
description: Use when reviewing LinkSys desired updates racing client publication or deletion.
knowledge_type: regression-guards
finding_id: ISSUE-001
---

# A superseded LinkSys publication keeps the runner alive

## Conclusion and scope

The Gateway's `PersistentRunner` is the per-`namespace/name` owner for Redis, Etcd,
Elasticsearch, Kafka, and OTLP clients. A newer `Desired.gen` invalidates only an
unpublished candidate; it does not close the mapped runner or discard the last
published `current` client. `DesiredState::NotReady` keeps that client. A real
delete or shutdown ends the runner; the separate `ReplaceNow` plan can
unpublish an unhealthy current client before building a replacement.

## Rationale and evidence

The lifecycle design's D2 requires stale initialization to be discarded while the
runner processes later desired input. D6 requires `NotReady` to retain the last
published handle (`tasks/todo/linksys-runtime-apply-lifecycle/10-decisions.md`).
`edgion-gateway/src/link_sys/runtime/store.rs` reuses a mapped runner on later
apply; it does not respawn a runner whose task exited unexpectedly.

ISSUE-001 found that `runner.rs::publish` returned one boolean for both a newer
desired generation and cancellation. `connect_generation` mapped either failure
to `LoopExit::Shutdown`, and `run_loop` called `finish_delete`. A post-swap
generation mismatch also cleared `current` and retired both the candidate and
the previous client. An ordinary update could therefore leave a stopped runner
in the typed map, with no client or path to recover on relist.

`publish` now returns `LoopExit::Aborted` for a superseded candidate and
`LoopExit::Shutdown` only for cancellation. The caller retires and shuts down
an unpublished candidate outside the publication lock, then continues the loop
on `Aborted`. The publication lock orders `publish` against `begin_delete`; a
held desired watch read guard orders the generation check and swap against
`send_modify` posts. No post-swap rollback is needed: a post either precedes
the publication check or follows the complete publication.

## Alternatives and limits

A post-swap rollback cannot safely restore the previous client after the new
client has become visible, and treating all failures as shutdown strands the
mapped runner. The correction affects the shared persistent runner, not the
Webhook publication path. It does not address separate full-set recreation,
forced-rebuild, or provider-specific health races tracked by other findings.

## Verification and re-evaluation

The `runner.rs` fake-owner test `superseded_at_publish_keeps_old_client_and_runner`
deterministically injects a generation change after the final init check. The
`desired_post_during_swap_keeps_published_client_and_runner` and
`delete_during_swap_unpublishes_the_new_client` tests use post-swap hooks and
check the resulting state. Their timeout-based blocking checks depend on thread
scheduling and do not prove that the other thread reached the lock before the
timeout. The focused runner test command passed 12 tests on 2026-09-17. These
unit tests do not establish live backend availability or every scheduler order.
Re-evaluate when changing the desired channel, publication lock order, deletion
path, or runner-map reuse policy.

## Resolution references

- `ISSUE-001` — corrected a high-severity stale-publication exit in the shared
  persistent runner on 2026-09-17.
