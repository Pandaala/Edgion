# Evaluate an ASCII fast path for non-SSE usage character counting

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `a7c2114f6`.

## Evidence and candidate

`ai/accounting/usage.rs::Utf8CharCounter::feed` visits every byte through its
incremental scalar state machine. `UsageExtractionProcessor::observe_chunk` calls it
for `ResponseStreamKind::None` when `responses` is false. The selective JSON
observer still runs separately. SSE uses its own data-character counter; native
Responses accounting does not use this estimate.

A possible fast path is `pending_len == 0 && bytes.is_ascii()`, returning the
chunk length converted to u64 with saturation, otherwise retaining the current
loop. This still scans the input; it replaces per-byte state transitions for
all-ASCII chunks. Mixed chunks can pay an extra prefix/full ASCII scan before the
existing loop. The zero-pending gate is essential: after an incomplete multibyte
sequence, ASCII may invalidate earlier continuation bytes and increase the count
by more than this chunk's length.

## Exploratory measurement

A temporary standalone Rust program extracted the production counter and compared
it with only that fast path added. It used `rustc --edition=2021 -O`, black-boxed
input slices and output counts, and processed about 16 million bytes per case.
Each case repeatedly fed one 16-, 1024- or 16384-byte chunk containing ASCII `a`,
or that chunk with `é` replacing the first or last two bytes. Three repeated runs
on this macOS arm64 workspace produced these elapsed ranges in milliseconds:

| Chunk bytes / contents | Existing | Candidate |
| --- | ---: | ---: |
| 16 / ASCII | 18.88–25.77 | 0.94–1.27 |
| 16 / multibyte at end | 21.50–22.51 | 23.66–24.61 |
| 1024 / ASCII | 18.36–19.22 | 0.547–0.549 |
| 1024 / multibyte at end | 18.42–18.93 | 18.98–19.56 |
| 16384 / ASCII | 18.40–18.56 | 0.460–0.473 |
| 16384 / multibyte at end | 18.20–18.79 | 18.77–19.35 |

These are exploratory counter-only timings, not statistically established
regressions or request-throughput gains. Existing ran before candidate each time;
there was no randomized ordering, CPU isolation, real provider corpus or complete
callback benchmark. JSON parsing, publication and quota work were excluded. The
probe did not validate split/incomplete-input equivalence. Reproduce from the
counter definition above with the stated one-branch modification and workloads;
temporary probe files are not maintained benchmark infrastructure.

## Decision and future acceptance

Keep the existing loop, or investigate the fast path after measuring actual
chunk sizes and ASCII/multibyte distributions. Compare small, large, empty,
ASCII-heavy, multibyte-heavy and malformed inputs with repeated interleaved trials,
then measure the full usage callback including its existing JSON observer. An
ASCII gain alone does not justify an unconditional extra mixed-input scan.

Before implementation, compare per-chunk and cumulative counts, live pending
state, abort-without-EOS and idempotent finish against the old state machine over
all split positions of representative permitted UTF-8 and malformed sequences.
Retain immediate leading-byte accounting, invalid-byte recovery, saturation and
bounded state. Do not change SSE, reported-only usage, quota or JSON validity
semantics. No new hot-path instrumentation is required.

Fresh unchanged-runtime verification passed 52 usage-accounting and 13 finalization
tests. These are baseline checks, not validation of the candidate. Runtime code
and tests remain unchanged pending a decision.
