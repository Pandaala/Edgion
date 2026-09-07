---
name: trailer-done-terminal-body-dispatch
description: Historical review evidence for Gateway / pingora fork response pumps; revalidate against current code before applying.
knowledge_type: regression-guards
---

# Trailer done terminal body dispatch

## Recorded conclusion

Fixed: `upstream_response_body_filter` now receives `end_of_stream = true` exactly once for every normal termination, including an H2 response ending in trailers and one ending in a bare `Done` — previously neither reached the body filter, so a withholding processor (Anthropic `process_non_streaming`, Guardrail `RawWindow`) silently dropped the whole response. A per-response `TerminalBodyDispatch` latch picks the single dispatching task; released bytes are emitted and cache-admitted ahead of the terminating task. `Failed` claims the latch without dispatching. Adjacent P4 observation gap in the DOWNSTREAM `response_body_filter` tracked separately in `tasks/todo/downstream-response-body-filter-trailer-eos-gap.md` — that hook is documented observation-only, so byte-loss re-reports against it are not defects.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

External reference: [h2-grpc/trailer-done-terminal-body-dispatch.md](https://github.com/Pandaala/pingora/blob/edgion_v3/edgion-changes/review/h2-grpc/trailer-done-terminal-body-dispatch.md)

## Resolution references

- `h2-trailer-eos-bypasses-response-body-filter` — 2026-08-24.
