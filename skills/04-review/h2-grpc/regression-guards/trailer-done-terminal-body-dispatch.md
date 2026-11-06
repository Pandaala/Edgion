---
name: trailer-done-terminal-body-dispatch
description: Historical review evidence for Gateway / pingora fork response pumps; revalidate against current code before applying.
knowledge_type: regression-guards
---

# Trailer done terminal body dispatch

## Recorded conclusion

Fixed: `upstream_response_body_filter` now receives `end_of_stream = true` exactly once for every normal termination, including an H2 response ending in trailers and one ending in a bare `Done` — previously neither reached the body filter, so a withholding processor (Anthropic `process_non_streaming`, Guardrail `RawWindow`) silently dropped the whole response. A per-response `TerminalBodyDispatch` latch picks the single dispatching task; released bytes are emitted and cache-admitted ahead of the terminating task. `Failed` claims the latch without dispatching. The adjacent P4 downstream observation fix and its contract boundary are recorded below.

## Evidence boundary

### Current dependency adoption

Cargo.lock now selects fork
`edgion_v0.9.1@e5f4287be713d01ffd444544251bf75c9d00062a`, which includes
downstream terminal observation; this was re-verified by source inspection at
that revision on 2026-11-06 after the fork was rebased onto upstream 0.9.1.
`TerminalBodyDispatch` is defined in `pingora-proxy/src/response_terminal.rs`
(unchanged from `0b0d8bba`; `Failed` still claims without dispatching) and
re-exported by `response_pipeline.rs`, whose `ResponsePipelineState` owns both
the upstream `terminal_body` latch and a separate `downstream_terminal_body`
latch; a `Failed` source also claims the downstream latch.
`lib.rs::downstream_response_body_filter_tasks` calls response_body_filter with
None and EOS true when a non-body task claims terminal observation, and rejects
nonempty synthetic output. The old nonadoption/open-gap statements below describe
the September snapshot, not the current dependency. This is source verification,
not fresh execution of fork regressions or proof of every transport interleaving.
Historical: the same adoption was first source-verified at Edgion `2bf6b8570`
against fork `1aaa45cc70430bc782990bc20ddee3fef36d6b99`.

### Downstream observation follow-up

At the historical September snapshot,
`downstream-response-body-filter-trailer-eos-gap` was implemented locally in the
sibling Pingora worktree based on `0b0d8bba` after explicit user authorization
on 2026-09-25. The canonical
fork record at `edgion-changes/review/h2-grpc/trailer-done-terminal-body-dispatch.md`
(`Downstream terminal observation` section)
owns the separate downstream latch, empty terminal callback, and rejection of
nonempty synthetic output. This task corrects an observation gap; no production
byte-loss impact was established. Edgion has no production override of this downstream hook, and its
lockfile then selected `7c24c0dd`; the local fix had not yet been adopted or published.

After independent review and targeted verification, the user explicitly
requested removal of the original task despite the unrelated baseline test
failures. Three default-retry unit tests still fail on the clean Pingora
baseline with the same dependency lockfile and remain tracked in the
separate baseline investigation at
`edgion-changes/pending-issues/default-retry-policy-test-baseline.md`.
Task removal does not claim a green full-library suite or dependency adoption.
Do not interpret the older upstream/compression closure evidence below as a
successful check of this local downstream change.

The hook permits observation and representation-preserving transformations.
Buffering by itself does not imply a total-length change, so the original
blanket dismissal of withheld-byte reports was too broad. The newly synthesized
Trailer/Done callback's no-output rule is an explicitly approved boundary;
deferred output support would require a separate design. Evaluate any future
buffering report against its actual framing and lifecycle instead of treating
the observation-only scope of this fix as the entire hook contract.

### Historical evidence

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
the unchanged trailer. The context's finalization flag (the unnamed second
tuple field of `ResponseCompressionCtx`, set once `response_body_filter` sees
`end`; there is no field literally named `finalized`) prevents the later
`Done` from producing another footer; the response-wide terminal latch prevents
a second terminal callback across pump batches.

The canonical fork explanation is
`edgion-changes/review/compression/compression-trailer-terminal-order.md`.
Historical source revalidation on 2026-09-09 compared Edgion's locked fork revision
`518f7274` with sibling checkout `dba39b6`; the compression implementation,
H2/custom pumps, and relevant integration test files are identical. This is
an already-fixed fork defect, not a new Edgion runtime change. At
`edgion_v0.9.1@e5f4287` (source inspection, 2026-11-06),
`pingora-core/src/protocols/http/compression/mod.rs` is unchanged from
`0b0d8bba` and still provides `response_filter_with_preceding` and the core
footer test; `pingora-proxy/tests/test_terminal_body_dispatch.rs` still
contains both H1/H2 `compressed_trailered_response_is_complete_and_dispatches_eos_once`
cases, and `pingora-proxy/tests/test_upstream_response_body_sink.rs` still
contains the custom-pump `custom_compressed_trailered_response_is_complete_and_dispatches_eos_once`.

Coverage includes `compression_footer_is_emitted_before_trailers_and_only_once`,
the H1/H2 `compressed_trailered_response_is_complete_and_dispatches_eos_once`
cases, and the custom-pump equivalent. The core test checks that concatenated
compressed data and footer fully decompress and that the following `Done`
stays unchanged. The response-wide context/latch lifetime supplies the
same-batch/split-batch ordering argument; this does not claim a dedicated
timed split-batch gzip end-to-end test.

Re-evaluate when compression finalization, prefix dispatch, batch reset,
terminal latching, or cache ordering changes. The separate downstream
observation gap was still open at that historical verification point; current
dependency adoption is recorded above.

Fresh closure checks on 2026-09-09: the sibling fork's focused H1/H2 gzip
trailered integration filter passed 2 tests; the custom-pump equivalent passed
1 test; the core compression footer test passed 1 test. Edgion format,
workspace check, Clippy, agent-doc, SSA-force, metrics-inventory, and
Pingora-source checks passed; Clippy reported existing warnings. No full
integration suite or deployed Gateway compression behavior is claimed.
Independent read-only closure review returned LGTM; the stale open task was
removed after these checks.
