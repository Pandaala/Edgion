---
name: cache-server-apply-change-info-level
description: Use when reviewing findings that ServerCache::apply_change emits per-resource INFO logs and should be downgraded to debug! — the INFO level is intentionally retained as a distinct pipeline-layer signal; only the type_name field was changed to kind_name.
---

# `ServerCache::apply_change` per-change INFO is intentionally kept at INFO

## Conclusion

`CacheEventDispatch::apply_change` (`edgion-controller/src/conf_sync/cache_server/cache.rs`)
emits one `tracing::info!(component = "cache_server", event = "apply_change", ...)` per
resource change (InitAdd / EventUpdate / EventDelete). A review (controller-05, review-2)
reported this as INFO log spam and proposed downgrading it to `debug!` (or splitting it by
change type). **The downgrade is not accepted — the line stays at INFO by deliberate
decision (controller-05, 2026-05-24).** Only the `kind` field was changed from
`std::any::type_name::<T>()` to `T::kind_name()` (the human-readable resource kind already
exposed via `ResourceMeta` / the `WatchObj` impl at `cache.rs:286`).

## Core Rationale

- The `cache_server` line is a **distinct pipeline-layer signal**: it confirms a change
  actually landed in the *served* `ServerCache` — the exact snapshot that gateways pull via
  the gRPC watch stream. Operationally, "the change reached the gateway-facing cache" is a
  meaningful checkpoint, separate from "the controller finished processing the resource".
- This is **not** the line-133 false-positive pattern in
  [observability-review-rule.md](observability-review-rule.md). That rule downgrades a
  per-event INFO that duplicates a **batch-summary** INFO aggregating the same keys
  (g9b-008, `cache_client`). Here the co-existing INFO is `process_resource` /
  `process_delete` — **per-resource spans**, not a batch summary — so the batch-vs-per-item
  downgrade rule does not apply.
- The `type_name` → `kind_name` change removes the only genuine noise the finding
  identified: the fully-qualified Rust type path (`crate::types::...::HttpRoute`) leaking
  into the `kind` field, which is unreadable for SREs. `kind_name()` yields the short,
  human-readable kind name.

## Fix Suggestions Not Accepted

- "Downgrade the per-change `apply_change` INFO to `debug!`" — **not accepted.**
- "Split by change type: keep EventDelete/EventUpdate at INFO, push InitAdd to `debug!`" —
  **not accepted** (the operator wants a uniform per-change checkpoint at this layer).
- Any re-report of "cache_server apply_change INFO floods logs during init/reload, should be
  debug" closes against this entry.

## Re-evaluation Triggers

- A concrete operational report that this specific line (not controller logging in
  aggregate) is the dominant cost in a real cluster's log bill.
- A future redesign that makes `process_resource`/`process_delete` carry the
  "landed in served cache" guarantee, making this line truly redundant.

## Reference Cases

- controller-05 (review-2, 2026-05-24): type_name → kind_name applied at all three
  `apply_change` log sites (per-change line + InitStart + InitDone); INFO level retained.
- g9b-008: the genuine batch-vs-per-item downgrade case (`cache_client`), recorded in
  [observability-review-rule.md](observability-review-rule.md) line 133 — contrast with
  this entry.
