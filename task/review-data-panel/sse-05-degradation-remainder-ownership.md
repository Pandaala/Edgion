# SSE degradation remainder ownership

Status: deferred for user decision. No runtime change.
Source baseline: `eb9997905`.

## Evidence and decision

`SseDecoder::process_bytes` owns the incoming Bytes, but passes a borrowed suffix
at its five calls to `degrade`. For a nonempty remainder, `degrade` copies that
suffix into a new Bytes and temporary SliceChain before publishing Opaque. It
publishes the previously admitted block first, then the remainder, and resets the
block. The separate end-of-feed normalization failure passes an empty remainder.

This cost occurs at the transition into permanent degradation, at most once per
decoder. Already-degraded `feed_records` shares the incoming Bytes directly.
BOM-replay failure also shares its separate remaining chunk through that caller.
Do not describe this as a payload copy on every degraded chunk.

Native Anthropic observation accepts only Block records; the conversion's
`feed_each_bytes` adapter likewise discards Opaque. Usage observation without an
active semantic guard ignores these records. The transition copy is therefore
discarded for those consumers. Conversion still fails on decoder degradation;
ignoring the raw record does not mean ignoring the failure.

The canonical guard path instead calls `CanonicalRecordBatch::push_observed`,
then `WindowRecord::from_sse_record`, which clones raw slice handles into owned
records. A shared suffix could keep a larger source allocation alive through
batch publication and failure/output handling. Logical raw length and metadata
admission do not prove identical backing-storage lifetime. The current compact
remainder copy isolates this new owner from source slack; it does not establish
that every other raw record is compact or that caller-owned backing is bounded.

Compare these alternatives before implementation:

- Retain the compact copy: limited failure-transition cost, unchanged ownership.
- Pass a shared Bytes suffix through the existing canonical owner. Measure saved
  copying against source backing retained by the batch, including small tails in
  large allocations. Preserve independent admission and peak-memory contracts.
- Support a synchronous borrowed raw record and materialize only when a consumer
  retains it. This changes the canonical adapter API and needs a reviewed lifetime
  and capacity design; do not add plugin-specific decoder or fallback mechanisms.

No option is selected. Source inspection establishes a copy, not production
frequency, CPU cost or a net throughput/RSS gain. This precedes
[sse-02](sse-02-fallback-record-handoff.md)'s batch fallback handoff and is distinct
from event-envelope serialization, semantic projection and dispatch metadata.

## Acceptance before implementation

Preserve raw bytes, record boundaries/order, admitted-block then rejected-tail
publication, sticky degradation, malformed diagnostics, character accounting and
EOS behavior. Cover BOM replay, CR/LF continuation append rejection, logical and
metadata limit failures, empty remainders, and retained-storage normalization.
Exercise observer-only, conversion and active-guard consumers independently.

Account for source backing, compact payload, handle tables and simultaneous batch
and callback output. Cover small suffixes in large allocations, fragmented and
coalesced blocks, rejected batch publication, held-head failure, fail-open/closed,
terminal arbitration and cancellation. No callback borrow may survive an await;
owned slices may, so verify their release points. Preserve canonical body/window
ownership and all admission/error ordering. Add no logs, response BodyStorage,
second window or unbounded aggregation.

Measure copies, allocations, live capacities and failure-transition frequency
before claiming a gain. Use IMPL with a frozen implementation allow-list after
user selection; this task authorizes no runtime change.

## Sources and verification

Sources: `sse/mod.rs` (feed_records, process_bytes, degrade, feed_each_bytes),
`sse/chain.rs` (push, raw_slices), `sse/window.rs` (from_sse_record),
`ai/protocols/openai/canonical_sse.rs` (push_observed, push),
`ai/accounting/usage.rs` (observe_chunk), native Anthropic processor and
Chat-to-Anthropic streaming conversion, all under `edgion-gateway/src/`.
Original authority: canonical SSE substrate architecture and
`skills/04-review/sse/regression-guards/window-capacity-admission.md`.

Fresh unchanged-code verification passed 273 tests: 80 SSE, 52 usage, 38 native
Anthropic, 64 Chat-to-Anthropic conversion, 25 semantic response and 14 canonical
SSE cases. Formatting and workspace all-target check/Clippy passed with existing
warnings. No new tests, runtime/fixture changes, allocation probe, benchmark,
full suite or live integration ran. These tests establish the baseline only.
