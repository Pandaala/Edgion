---
name: trailer-batch-latch-cache-completion
description: Historical review evidence for Gateway / pingora fork cache admission; revalidate against current code before applying.
knowledge_type: regression-guards
---

# Trailer batch latch cache completion

## Recorded conclusion

No product change: the claimed hang (a batch-ending `Trailer` latches `upstream_done`, stranding the `Done` that runs `finish_miss_handler()`, so a streaming miss body reader waits forever) is unreachable — every pump takes an early `continue` before `maybe_set_upstream_done(source_done)` whenever a cache readback is on, and a streaming miss is exactly what turns it on. Measured with an instrumented pump on the finding's own forced `[H]` / `[B,B,B,T]` / `[D]` layout: `Done` is still delivered, and an end-to-end test built on that layout passes identically with and without the proposed `Trailer` arm. Residual gap exists only for a non-streaming storage backend (none in this tree, `MemCache` is always streaming) and is a silently uncached response, not a stall or a truncated entity.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

External reference: [h2-grpc/trailer-batch-latch-cache-completion.md](https://github.com/Pandaala/pingora/blob/edgion_v3/edgion-changes/review/h2-grpc/trailer-batch-latch-cache-completion.md)

## Resolution references

- `issue-trailer-done-cross-batch-cache-eof` — 2026-08-24 (❌ premise disproven).
