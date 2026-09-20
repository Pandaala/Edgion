---
name: full-set-retain-serializes-join-replay
description: Use when reviewing LinkSys full-set omission, kind changes, pending recreation, and delete join replay for persistent providers.
knowledge_type: regression-guards
finding_id: ISSUE-002
---

# Full-set omission cannot resurrect removed LinkSys resources

## Conclusion and scope

The unified-table refactor replaces the per-provider apply locks and pending
recreation maps described below. Full-set omission now removes the current typed
entry and signals its provider to stop. Cleanup never mutates the registry or
replays creation, so there is no delete-join replay to serialize. A type change
removes the old current entry before admitting the new provider, without waiting
for old cleanup. See [simple removal](../intentional-design/provider-owned-runtime-and-simple-removal.md).

The remaining sections document historical fixes and tests of the removed
implementation, not requirements to retain its locks or pending maps. Preserve
the behavioral regression boundary: an omitted key cannot reappear from stale
cleanup, and unrelated ordered deltas must not be lost.

## Historical implementation and evidence

For Redis, Etcd, Elasticsearch, Kafka, and OTLP, a LinkSys key omitted from an
authoritative full-set must have neither a mapped persistent runner nor a
pending recreation after that provider's retain step. Each provider's
`APPLY_LOCK` spans runtime key enumeration, deletion of omitted runners, and
pending pruning. The `ProviderRuntimeStore.map` holds runners keyed by
`namespace/name`; the provider's `PENDING` map holds a `LinkSys` resource and
forced-rebuild flag while an earlier runner is joining.

For a full-set that changes a key from one persistent provider kind to another,
all five providers' retain steps must finish before any provider applies the
incoming resource. This removes the old kind's callable runner before the new
kind can start. It does not require the old runner's shutdown join to finish.

## Rationale and evidence

The LinkSys runtime apply contract defines full-set omission as Delete and
requires full-set retain to drop pending recreation for absent keys
(`tasks/todo/linksys-runtime-apply-lifecycle/00-spec.md` and
`12-independent-review.md`). `LinkSysStore::replace_all` sends the source
snapshot to `dispatch_full_set`, which constructs each provider's present-key
set and calls its `retain_*_runners` function. A delete joiner can independently
take `APPLY_LOCK`, remove `PENDING`, and call `apply_*_locked` to recreate the
runner. Source dispatch ordering does not serialize that joiner.

ISSUE-002 identified a gap: retain enumerated runtime keys before taking
`APPLY_LOCK`. A joiner could recreate an omitted key after enumeration but
before pending pruning, leaving the key mapped. The retain functions now use
`retain_persistent_runners`, which takes the provider lock before enumeration
and keeps it through deletion and pending pruning. Their deletion callbacks
call the existing `delete_*_runner_locked` helpers to avoid reacquiring the
same lock.

ISSUE-005 identified a separate ordering gap in the same full-set dispatch:
the input loop applied new persistent kinds before the provider retain steps.
A Redis-to-Kafka change could therefore start Kafka while the old Redis client
remained available through its typed getter. `dispatch_full_set_with` now
collects the five present-key sets, retains each provider, and only then applies
the input resources. This follows the LinkSys D7 kind-change contract in
`tasks/todo/linksys-runtime-apply-lifecycle/10-decisions.md`.

## Alternatives and limits

An `ArcSwap` runtime map makes individual updates atomic but does not make
enumeration and pending pruning one operation. This guard applies to the five
persistent providers; Webhook uses a separate publication path. The unit test
checks lock coverage and omitted-key cleanup in the shared helper. It does not
exercise live provider connections or every scheduler ordering.
The kind-change test blocks Kafka apply admission and checks that the old Redis
runner and getter are already empty. It covers persistent-provider ordering,
not the separate Webhook publication transaction or stronger cross-provider
shutdown fencing.

## Verification and re-evaluation

`cargo test -p edgion-gateway --lib full_set_retain_holds_apply_lock_through_runtime_and_pending_cleanup`
passed on 2026-09-17. Re-evaluate if a provider stops using the shared retain
helper, moves join replay outside its `APPLY_LOCK`, or changes the full-set
omission contract.
`cargo test -p edgion-gateway --lib dispatch_full_set_removes_old_kind_before_applying_new_kind`
passed on 2026-09-17. Re-evaluate the kind-change ordering if full-set dispatch
applies a provider before all persistent retain steps complete.

## Resolution references

- `ISSUE-002` — corrected the full-set omission versus delete join replay race
  on 2026-09-17.
- `ISSUE-005` — corrected full-set persistent kind-change ordering on
  2026-09-17.
