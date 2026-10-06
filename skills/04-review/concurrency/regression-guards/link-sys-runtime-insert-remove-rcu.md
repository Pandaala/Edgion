---
name: link-sys-runtime-insert-remove-rcu
description: LinkSys runtime mutations remain serialized; current unified-registry locking replaces the historical per-provider ArcSwap RCU mechanism.
knowledge_type: regression-guards
---

# LinkSys runtime mutation preserves concurrent updates

## Current conclusion

The unified `RuntimeRegistry` in `runtime/store.rs` has one authoritative write
boundary: `LINK_SYS_SOURCE_EVENT_LOCK` spans source snapshot construction, map
cloning, side-effect replacement, runtime publication, and subsequent raw source
publication. Rebuilds and admission retries use the same lock. OTLP process
shutdown also takes it before setting `OTLP_STOPPING`, removing current OTLP
owners, and transferring retained cleanup tasks; cleanup waits occur afterward.
Preparation performs no network IO; persistent provider connection workers
remain asynchronous. Reads load the ArcSwap without taking the source-event lock.
Both full and partial updates obey this order; an older full set cannot overtake
a later delta.
The source snapshot also retains capacity-denied intent for ordered retry.

This preserves the original no-lost-update invariant without the deleted
`ProviderRuntimeStore` or its RCU helpers. See
[provider-owned runtime](../../link-sys/intentional-design/provider-owned-runtime-and-simple-removal.md).
Relevant current tests include
`synchronous_dispatch_publishes_before_source_and_preserves_unrelated_deltas`,
`source_events_serialize_full_sets_deltas_rebuilds_and_admission`, and
`otlp_shutdown_serializes_with_source_updates`. Scoped threads gate actual source
operations; the shutdown test runs in a child process because stopping is terminal
global state. These are ownership/ordering tests, not live SDK integration evidence.
The former async dispatch tail was removed by
[prepared HTTPDNS publication](../../link-sys/regression-guards/httpdns-prepared-runtime.md);
its task-chain tests do not establish a current implementation requirement.

The user-authorized P3 maintenance task `simplify-linksys-runtime-ordering`
removes the redundant global dispatch counter, overflow rejection, stale-sequence
comparison, and registry mutex. Production dispatch is synchronous under the
source-event lock, so obsolete out-of-order calls to the private registry are not
a production input. Test-only global installations take the same source-event
lock. Provider-local `Desired.gen`, `force_gen`, and publication/delete fences
remain necessary for asynchronous connection work and are unchanged.
Controller's one worker per kind and Gateway's per-kind cache write lock order
normal configuration callbacks; neither alone orders other-kind dependency
rebuilds or process shutdown. Do not remove the source-event lock on that basis.

Re-evaluate this boundary if dispatch becomes asynchronous, a registry writer
bypasses the source-event lock, or shutdown/cleanup acquires locks in reverse
order. Keep unrelated deltas, latest-snapshot admission, and no OTLP creation
after stopping as the behavioral guards, rather than preserving a particular
dispatch counter implementation.

Fresh verification for `simplify-linksys-runtime-ordering` on 2026-10-06:
`cargo test -p edgion-gateway --lib link_sys::runtime -- --test-threads=1`
passed all 82 tests, and independent Astra source/knowledge review returned LGTM.
`cargo test -p edgion-gateway --features test-util --test otlp_transport`
passed all nine real transport tests, including shutdown/update interleaving.
The unit results cover ordered publication and runner fences; the transport
results add SDK lifecycle evidence, not a global lossless delivery guarantee.
The startup-drain test and both Webhook manager tests also passed. Workspace
format/check/Clippy and the SSA, metrics, Pingora source, Gateway API transform,
unit-test layout, gateway tracing, and English-only guards passed. The unrelated
WAF task-link blocker was resolved, and agent-doc validation passed again at
closure on 2026-10-06. The user's shared-topology integration report
(`tmp/integration_testing/testing_20261006_121044/report.log`) records 257 passed,
seven skipped, zero failed and zero timed out, including LinkSys replacement
and disabled-TLS dynamic scenarios. The user authorized task closure and deletion.

## Historical RCU implementation (inactive)

All implementation claims, rejection rules, and scope notes below describe the
removed per-provider registries. They do not reject the current write-only mutex
or authorize an older full set to overwrite a newer delta.

**Status:** ✅ fixed · 2026-05-25
**Severity:** Medium (BadLogic) — non-atomic RMW on the process-global runtime maps could silently lose a concurrent insert/remove

> Not to be confused with the link-sys-F001 controller path — that finding described the
> controller-side webhook endpoint injection (`handlers/link_sys.rs`) and has since been
> entirely removed by the 2026-06-20 endpoint-decouple refactor
> (`docs/superpowers/specs/2026-06-20-linksys-webhook-endpoint-decouple-design.md`).
> This entry is the **gateway** runtime store (`edgion-gateway/src/link_sys/runtime/store.rs`).

## Conclusion

As of F-007 (2026-05-25) the four per-provider `*_runtime_insert` / `*_runtime_remove` helpers
were consolidated into a single generic `ProviderRuntimeStore<C>` newtype in
`edgion-gateway/src/link_sys/runtime/store.rs`; every provider static (`REDIS_RUNTIME`,
`ETCD_RUNTIME`, `ES_RUNTIME`, `KAFKA_RUNTIME`, `WEBHOOK_RUNTIME`) is a
`ProviderRuntimeStore<_>`. Redis, Etcd, Elasticsearch, and Kafka store `PersistentRunner`
values; Webhook stores the published client. Its
`insert` / `remove` methods perform the map mutation through `ArcSwap::rcu`, which retries the
load-clone-modify closure on CAS contention. The mutation is therefore atomic against concurrent
dispatch tasks — the previous unguarded `load → clone → insert/remove → store` could lose an
update.

- **`ProviderRuntimeStore::insert`**: `self.map.rcu(|current| { let mut m = (**current).clone(); m.insert(key.clone(), client.clone()); m })`.
  The closure is `FnMut` and may re-run, so `key` / `client` are cloned per iteration.
- **`ProviderRuntimeStore::remove`**: a `self.map.load().contains_key(key)` fast path returns `None` without cloning when
  the key is clearly absent; otherwise `let old = self.map.rcu(|current| { let mut m = (**current).clone(); m.remove(key); m }); old.get(key).cloned()`.
  `rcu` returns the map as it was immediately before the successful swap, so `old.get(key)` is
  exactly the client this call removed.

`ProviderRuntimeStore::replace_all` was **not** changed in spirit — it is a single `ArcSwap::swap`, already atomic.

Re-reports of the form "the runtime-store insert does a non-atomic read-modify-write with no
CAS / a full resync and an incremental update race and overwrite each other / the remove
has the same lost-update bug" are **not accepted** — verify the current
`ProviderRuntimeStore::{insert,remove}` use `rcu` before re-reporting.

## What was wrong (before the fix)

Each helper ran `let current = RUNTIME.load(); let mut m = (**current).clone(); m.insert(..);
RUNTIME.store(Arc::new(m));`. `LinkSysStore::update()` spawns each `dispatch_partial_update` as
an independent `tokio` task, so two incremental updates close in time could both snapshot the
same base map, each apply their own inserts/removals, and the second `store()` silently discard
the first writer's change. The lost update made `get_redis_client("ns/name")` (or the Etcd / ES
equivalent) return `None` after an insert that appeared to succeed; the orphaned client kept
running its background `init()` but was unreachable through the public API until the next full
resync.

## Why the fix is correct

`rcu` performs `compare_and_swap` and re-runs the closure whenever the pointer changed between
load and swap, so concurrent insert/remove writers serialize and the last one wins
deterministically with no lost update. This is the same defect class as
[backends-F002](../../backends/regression-guards/upsert-task-concurrent-handle-leak.md),
[backends-F004](../../backends/regression-guards/set-service-config-toctou-race.md), and
[backends-F005](../../backends/superseded/warm-cell-toctou-stale-overwrite.md) — non-atomic
multi-step mutation on a config-sync–driven shared store — fixed with the same
atomic-swap-with-retry principle. The remove fast-path `contains_key` check is a benign
optimization only: a concurrent insert landing after the check is a separate entry the call
legitimately does not remove.

## Fix Suggestions Not Accepted

- "Wrap all four runtime stores in a global `Mutex`/`RwLock`" — heavier than needed and would
  serialize the lock-free `get_*_client` reads that `ArcSwap` exists to keep wait-free; `rcu`
  fixes the write race without touching the read path.
- "Drop the remove fast-path `contains_key` check, always `rcu`" — the check avoids a map clone
  on the common no-op remove and does not affect correctness (the `rcu` closure is still the
  atomic point); keep it.

## Re-evaluation Triggers

Re-open only if:

- `ProviderRuntimeStore::insert` / `::remove` reverts to the unguarded `load`/`clone`/`store`
  form (reopening the lost-update window).
- A new provider bypasses `ProviderRuntimeStore` and re-introduces a hand-rolled non-atomic
  insert/remove instead of reusing the generic (which inherits `rcu` for free).

## Scope note — full-set vs partial swap is a separate concern

`dispatch_full_set` uses `ProviderRuntimeStore::replace_all` (an unconditional `ArcSwap::swap`), not the
insert/remove methods. `rcu` makes a concurrent partial insert retry on top of a full-set swap,
but if the full-set swap lands last it still overwrites the partial insert — this is the
intended "full resync is authoritative" eventual-consistency semantics of config sync, a
higher-level design property, not part of this insert/remove atomicity fix. A re-report scoped
strictly to "full swap can clobber a partial insert" is about that design property, not a
re-report of this finding.

## Reference Cases

- link-sys-F001 runtime-store (review finding, `tasks/link/`, 2026-05-25)
- backends-F002 / backends-F004 / backends-F005 — sibling fixes, same atomic-mutation principle
  on config-sync–driven maps.
- `edgion-gateway/src/link_sys/runtime/store.rs` — `ProviderRuntimeStore::{insert,remove}` (F-007
  consolidated the eight per-provider helpers into this generic) and `LinkSysStore::update` /
  `replace_all` that spawn the racing dispatch tasks.
