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

## Compression footer ordering

`issue-compression-trailer-double-eos` is also fixed. The former compression
path left `Trailer` unchanged and rewrote the following `Done` into a terminal
footer body, producing body bytes after trailers and a second body-hook EOS.
`ResponseCompressionCtx::response_filter_with_preceding` now finalizes before
the trailer and returns a non-terminal footer body. H1, H2, and custom pumps
process that prefix through the ordinary body-filter/cache pipeline before
the unchanged trailer. The context's `finalized` guard prevents the later
`Done` from producing another footer; the response-wide terminal latch prevents
a second terminal callback across pump batches.

The canonical fork explanation is
`edgion-changes/review/compression/compression-trailer-terminal-order.md`.
Source revalidation on 2026-09-09 compared Edgion's locked fork revision
`518f7274` with sibling checkout `dba39b6`; the compression implementation,
H2/custom pumps, and relevant integration test files are identical. This is
an already-fixed fork defect, not a new Edgion runtime change.

Coverage includes `compression_footer_is_emitted_before_trailers_and_only_once`,
the H1/H2 `compressed_trailered_response_is_complete_and_dispatches_eos_once`
cases, and the custom-pump equivalent. The core test checks that concatenated
compressed data and footer fully decompress and that the following `Done`
stays unchanged. The response-wide context/latch lifetime supplies the
same-batch/split-batch ordering argument; this does not claim a dedicated
timed split-batch gzip end-to-end test.

Re-evaluate when compression finalization, prefix dispatch, batch reset,
terminal latching, or cache ordering changes. The separate downstream
observation gap above remains open.

Fresh closure checks on 2026-09-09: the sibling fork's focused H1/H2 gzip
trailered integration filter passed 2 tests; the custom-pump equivalent passed
1 test; the core compression footer test passed 1 test. Edgion format,
workspace check, Clippy, agent-doc, SSA-force, metrics-inventory, and
Pingora-source checks passed; Clippy reported existing warnings. No full
integration suite or deployed Gateway compression behavior is claimed.
Independent read-only closure review returned LGTM; the stale open task was
removed after these checks.
