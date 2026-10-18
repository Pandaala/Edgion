# Incremental attempt-log size accounting

Status: deferred optimization for user decision. No runtime change.
Source baseline: 783721aa4.

## Evidence

AiAttemptRuntimeState::commit_ledger in
edgion-gateway/src/ai/runtime/attempt_runtime.rs updates usage, metric and adaptive
totals before log admission. It rejects entries beyond the eight-entry limit,
then pushes an entry and serializes the whole retained Vec through AttemptLogSize.
The writer counts bytes without allocating a JSON buffer and rejects writes beyond
MAX_AI_ATTEMPT_LOG_BYTES (8 KiB). Serialization failure pops only the candidate
and sets attempts_truncated; accounting still includes the omitted attempt.

A sequence of eight accepted entries visits 1 + 2 + ... + 8 = 36 entries across
these size checks. This is not the cost of every request. Normal dispatch is
bounded to eight attempts; the current scan is bounded and no CPU bottleneck has
been established. GenAiAttempt in ai/accounting/usage.rs uses derived Serialize
over strings, integers, enums and optional fields. Its escaped JSON size cannot
be replaced by a sum of raw string lengths.

An incremental alternative could serialize only the candidate using the same
serializer and counting writer, then combine its length with the retained sum,
two array brackets and the appropriate commas. This reduces repeated scans but
adds persistent request state and a cached-length consistency invariant. Measure
the actual struct/future footprint including padding rather than assuming one
counter translates directly to a fixed allocation increase. The current approach
already avoids temporary serialization allocations.

## Decision

Choose whether to retain the bounded whole-list check or evaluate incremental
size accounting with a benchmark and explicit state invariant. No throughput,
CPU or net memory benefit is established. This proposal does not cover taking
the ledger Vec into the final summary, admission ownership or log schema changes.

## Acceptance for a selected implementation

- Keep the existing serializer and exact wire format. Differentially compare
  cumulative lengths against serialization of the entire Vec after every step,
  covering empty/single/eight entries, optional fields, integer widths, escaped
  strings, permitted multibyte UTF-8 and cost strings.
- Preserve acceptance at exactly 8 KiB, rejection above it, the eight-entry cap,
  attempts_truncated and prior entries after rejection. Exercise independent
  byte-cap rejection with synthetic entries beyond normal field bounds.
- On serialization failure, byte overflow or count rejection, do not advance
  cached retained size. Keep usage, metric and adaptive accounting before log
  admission. Check a rejection followed by another candidate so stale counters
  cannot silently change future admission.
- Define the counter's initialization and every mutation/clear/move boundary.
  Keep it private to this owner; do not introduce another serializer, logging
  channel, request-path tracing or shared synchronization.
- Measure common one-attempt and retry-heavy cases, CPU, allocation counts and
  retained state size. Do not infer a win from the 36-to-8 visit count alone.
  Keep existing bounded identity handling and canonical access-log ownership.
- Use a new IMPL plan with an exact allow-list before implementation.

## Verification limits

The audit passed 25 attempt-runtime and 13 usage-finalization network-free tests,
formatting, workspace check/Clippy and agent-document/diff validation. These
verify the unchanged current path, not an incremental counter or measured runtime
benefit. Existing attempt_log_size_counts_wire_bytes_without_allocating covers
escaped output, exact encoded-size acceptance, zero counting allocations and
rejection without losing prior entries or accounting.
