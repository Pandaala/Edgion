---
name: webhook-manager-rwlock-intentional
description: Use when reviewing the LinkSys WebhookManager read path; explains why `WebhookManager::get` keeps a `tokio::sync::RwLock` instead of migrating to `ArcSwap` like the other runtime stores. Reject re-reports that the per-call `read().await` is an unnecessary hot-path overhead / pattern inconsistency.
knowledge_type: intentional-design
---

# WebhookManager keeps `RwLock` (not `ArcSwap`) on the read path — DESIGN-TRADEOFF

**Status:** ⚠️ accepted tradeoff · 2026-05-25
**Severity:** Low (BadLogic — pattern inconsistency / non-measurable overhead)

## Conclusion

`WebhookManager::services` is `tokio::sync::RwLock<HashMap<String, WebhookEntry>>`
(`edgion-gateway/src/link_sys/providers/webhook/manager.rs:55`) and `WebhookManager::get`
(`manager.rs:66-68`) takes `self.services.read().await` on every outbound call. This stays as-is.
The per-call read-lock is not a measurable cost and the `ArcSwap` migration the finding asks for is
structurally heavier than the codebase-wide pattern it cites.

Fix suggestions of the form "migrate `services` to `ArcSwap<HashMap<String, Arc<WebhookRuntime>>>`
+ a side `Mutex<HashMap<String, JoinHandle>>` for the health tasks, matching `store.rs`" and
"make `get` a synchronous lock-free fn" are **not accepted**.

## Core Rationale

**1. The overhead is non-measurable, by the finding's own admission.**

Reads take a shared read-lock, so concurrent `get` calls never block each other; writers
(`upsert` / `remove`, `manager.rs:83`/`198`) take the write-lock only on a LinkSys config change
(rare). The only per-call cost is the waiter node tokio's async `RwLock` allocates per
`read().await` even uncontended — nanosecond-range against the millisecond-range HTTP round-trip
that immediately follows. The issue file states it directly: "performance impact is not
measurable". A 🔴 Fix-Value: harm negligible, cost/risk out of proportion.

**2. `WebhookManager` is structurally different from the stores it is compared to.**

The other LinkSys runtime stores (`REDIS_RUNTIME` / `ETCD_RUNTIME` / `ES_RUNTIME` /
`WEBHOOK_RUNTIME` in `runtime/store.rs`) hold a single `Arc<XxxLinkClient>` per key, which adapts
cleanly to the `ArcSwap::rcu` clone-modify-store pattern (see
[link-sys-runtime-insert-remove-rcu.md](../regression-guards/link-sys-runtime-insert-remove-rcu.md)).
`WebhookEntry` (`manager.rs:48-51`) instead couples `runtime: Arc<WebhookRuntime>` **with a
non-`Clone` `health_task: Option<JoinHandle<()>>`** in one entry. A `JoinHandle` cannot live in an
`ArcSwap`-backed map whose mutation is "clone the whole map, modify, store" — it is not clonable.
The `RwLock` is what keeps "swap the runtime **and** abort the old health task" a single atomic
write. This is the reason it is on `RwLock`, not an oversight.

**3. `WEBHOOK_RUNTIME` already being `ArcSwap` is a different registry.**

`WEBHOOK_RUNTIME` (`store.rs`) tracks `Arc<WebhookLinkClient>` lifecycle/shutdown objects and does
use `ArcSwap`; `WebhookManager` is the second, separate registry that serves the plugin read path
(`get` / `get_webhook_call_handle`). They hold different value shapes for different purposes; the
manager being on `RwLock` is not an inconsistency within a single store.

## Fix Suggestions Not Accepted

- **"Migrate to `ArcSwap<HashMap<String, Arc<WebhookRuntime>>>` + side `Mutex<HashMap<String, JoinHandle>>`"**
  — splits one write-locked atomic "replace runtime + abort old health task" into two separately
  locked maps. That introduces a new cross-map consistency window (runtime swapped but health task
  not yet swapped/aborted, or vice versa) and a second lock to coordinate in `upsert`/`remove`, in
  exchange for a read-side gain the finding itself calls non-measurable. Net new concurrency risk
  for zero benefit.
- **"Make `get` a synchronous lock-free fn"** — ripples the `async fn get` signature change through
  every caller (`get_webhook_call_handle`, the `key_auth` webhook-key resolver) and only makes
  sense alongside the two-map split above, so it inherits the same rejection.

## Re-evaluation Triggers

Re-open this decision only if:

- Profiling shows webhook-key resolution as a real data-plane bottleneck — e.g. many `key_sources`
  entries per request, or webhook resolution moved onto a high-frequency per-request rate-limit
  check — so the per-call read-lock becomes measurable.
- `WebhookEntry` stops coupling the non-`Clone` `JoinHandle` with the runtime (e.g. health tasks
  get tracked elsewhere for an independent reason), which would make the `ArcSwap` migration a clean
  single-map change instead of a two-lock split.

## Reference Cases

- link-sys-F011 (review finding, `tasks/link/`, 2026-05-25).
- [lb-F010](../../lb/intentional-design/build-lock-single-mutex-intentional.md) — sibling concurrency
  accepted-tradeoff: a coarse lock kept intentionally because the finer-grained "fix" is heavier
  and riskier than the cold-path benefit.
- [link-sys-runtime-insert-remove-rcu.md](../regression-guards/link-sys-runtime-insert-remove-rcu.md) — the sibling
  `store.rs` stores that DO use `ArcSwap`, and why they can (single `Arc<Client>` per key, no
  coupled `JoinHandle`).
- `edgion-gateway/src/link_sys/providers/webhook/manager.rs` — `WebhookManager`, `WebhookEntry`,
  `get` / `upsert` / `remove`.

## Resolution references

- `link-sys-F011` — 2026-05-25 (⚠️ accepted tradeoff).
