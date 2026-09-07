---
name: conf-sync-batch-info-count-only
description: Use when reviewing config-sync batch logs — the batch-level `info!` MUST carry counts only; the full key list belongs at `debug!`. Established pattern in `cache_client/event_dispatch.rs:105` and applied in `cache_data.rs::process_compressed_events` (F-B01).
knowledge_type: mandatory-design
---

# Config-sync batch logs: `info!` is count-only, key detail is `debug!`

## Conclusion

Inside the gateway config-sync cache_client path (`edgion-gateway/src/conf_sync/cache_client/`),
any batch-level summary log of `add` / `update` / `remove` resource keys MUST follow this
shape:

- `tracing::info!(..., add_count = N, update_count = N, remove_count = N, "<summary>")`
- `tracing::debug!(..., add_keys = ?…, update_keys = ?…, remove_keys = ?…, "<detail>")`

The full `Vec<&String>` of keys never appears at `info` level. Per-item `debug!` for each
classified event (see `cache_data.rs` lines around the per-key debug calls) is also retained
because it already carries the same detail at the same level.

## Core Rationale

- Config-sync is debounced at 100 ms; during rolling updates a single batch can carry
  hundreds of distinct keys. Serializing the full key list with `?` Debug into an
  unconditionally-on `info!` allocates a string proportional to fleet size × kinds × churn,
  with no operational value beyond what counts already give.
- Resource keys for kinds like `EdgionTls` / `LinkSys` are sensitive-adjacent; keeping them
  out of permanent info-level logs is the safer default.
- The canonical pattern lives at
  `edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs:105` — `info!` carries
  `kind` + `count`, the matching `debug!` (lines 109-114) dumps the key vector. This entry
  generalizes that rule to all batch summaries in the same module.

## Fix Suggestions Not Accepted

- "Keep the full key list at `info` — operators want to see what changed." — **not
  accepted.** Operators get counts at `info` and can enable `debug` to drill in; the
  always-on cost is not justified.
- "Cap the printed key list at the first N entries and keep it at `info`." — **not
  accepted.** Capping hides arbitrary keys; the count-vs-detail split is cleaner and
  matches the established pattern.

## Reference Cases

- F-B01 (conf-sync, 2026-05-28): `cache_data.rs::process_compressed_events` batch `info!`
  changed from full key vectors to counts; full key dump moved to a sibling `debug!`.
- g9b-008: the dual-rule reverse direction — per-item `info!` co-existing with a
  batch-level `info!` carrying the same keys gets downgraded to `debug!`. Recorded in
  [observability-review-rule.md](observability-review-rule.md) line 134. The two entries
  together cover both directions of the batch/per-item info-vs-debug split.

## Re-evaluation Triggers

- A concrete operational report that count-only info is insufficient for diagnosing a
  real production incident that `debug` could not be enabled for in time.
- A redesign that introduces structured per-batch events on a dedicated channel,
  making the textual log line obsolete.
