---
name: stream-plugin-store-partial-update-per-key-debug-removed
description: Use when reviewing findings about per-key tracing::debug! inside StreamPluginStore::partial_update (or similar ConfHandler partial_update loops) — the per-key debug log is redundant when the function-entry info! already records the rm/add/update counts.
---

# `StreamPluginStore::partial_update` per-key `tracing::debug!` removed

## Conclusion

`StreamPluginStore::partial_update` (`edgion-gateway/src/plugins/stream/stream_plugin_store.rs`)
used to emit one `tracing::debug!(key = %key, "Removing EdgionStreamPlugins")` per removed
key in a loop, right after a batch `tracing::info!(rm = remove.len(), ...)` at function
entry. **The per-key debug loop was removed (plugins-stream F-010, 2026-05-27).** The
function-entry `info!` already records the full `add / update / rm` counts, and the per-key
debug line adds no structured information that the batch summary does not already convey;
under `RUST_LOG=debug` during large reconcile events (namespace teardown with 50+
`EdgionStreamPlugins` resources) it became log spam that buried more useful entries.

## Core Rationale

- This is a **count-aggregated batch summary vs. per-key debug** variant of the pattern
  documented at [observability-review-rule.md](observability-review-rule.md) row 134
  (g9b-008 in `cache_client`). g9b-008 downgrades per-event `info!` → `debug!` when a
  batch `info!` already aggregates the same keys. F-010 takes the next step on the same
  axis: when the per-event line is already at `debug!` **and** the batch line only carries
  the count (not the individual keys), the per-key debug is still redundant — a bare key
  with no context (no replacement value, no prior version, no cause) is not actionable on
  its own. If per-key detail is ever needed, prefer a single structured span over the
  whole `partial_update` call, not N individual log entries.
- The fix is purely observational: `self.update(add_or_update, &remove)` still runs
  unchanged, so the store mutation and downstream behavior are not affected.
- Contrast with [cache-server-apply-change-info-level.md](cache-server-apply-change-info-level.md):
  that case kept INFO because the per-change line is a distinct pipeline-layer signal
  (confirming the change landed in the served cache). Here there is no such second-layer
  guarantee — the debug! line is purely informational and the batch `info!` already
  records the only field (`rm` count) that has operational value.

## Fix Suggestions Not Accepted

- "Keep the per-key debug for traceability, just gate it on a sampling switch" — **not
  accepted.** Any re-report proposing to reintroduce per-key logging in
  `partial_update` (debug or info, sampled or not) closes against this entry unless a
  concrete debugging gap is demonstrated that the batch counts cannot answer.

## Re-evaluation Triggers

- A concrete debugging case where knowing exactly which keys were removed (without other
  context) would have shortened MTTR.
- A redesign where `partial_update` carries enough surrounding context per key
  (replacement source, prior generation, cause) that per-key logging is no longer a bare
  key dump.

## Reference Cases

- plugins-stream F-010 (2026-05-27): per-key `tracing::debug!` loop deleted; the
  function-entry `tracing::info!` with `add / update / rm` counts is the sole observability
  hook for `partial_update` in `StreamPluginStore`.
- g9b-008: the original batch-vs-per-item downgrade case in `cache_client`, recorded in
  [observability-review-rule.md](observability-review-rule.md) row 134 — F-010 is the
  count-only batch variant of the same family.
