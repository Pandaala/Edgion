# Evaluate gRPC streaming terminal record storage

Status: deferred user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `07e4bb3da`. No runtime implementation selected.

## Evidence and scope

`edgion-gateway/src/plugins/http/grpc_json_transcoder/response.rs::terminal`
constructs the final NDJSON or SSE record. Success and missing/invalid status use
literal bytes copied into a Vec. Nonzero status uses format! followed by
into_bytes. Production status validation accepts only canonical codes 0 through
16; the error text is fixed and never copies upstream grpc-message content.
This is one terminal record per finished streamed exchange, not per data chunk.
Unary fixed errors and normal message serialization are outside this candidate.

The streaming settle branch checks record length against the configured JSON
limit before GrpcTranscodingPlan::output forwards the Vec to Reservation::output.
That owner charges Vec capacity against the 1 MiB outstanding-output budget and
retains exchange admission through Bytes::from_owner until the last output
reference is dropped. Shorter capacity can therefore affect admission when earlier
output remains outstanding, even when the emitted bytes are identical.

A standalone probe extracted the current terminal function unchanged, supplied
the two format enum variants, and called it for None and every code 0 through 16.
On rustc 1.96.1, aarch64-apple-darwin, it observed:

| Format | Status | Length | Vec capacity | Slice-to-Vec copy capacity |
| --- | --- | ---: | ---: | ---: |
| NDJSON | None | 59 | 59 | 59 |
| NDJSON | 0 | 15 | 15 | 15 |
| NDJSON | 1–9 | 74 | 146 | 74 |
| NDJSON | 10–16 | 75 | 146 | 75 |
| SSE | None | 68 | 68 | 68 |
| SSE | 0 | 25 | 25 | 25 |
| SSE | 1–9 | 83 | 164 | 83 |
| SSE | 10–16 | 84 | 164 | 84 |

These are this build's logical lengths/capacities, not allocator usable sizes,
allocation counts, peak heap, CPU or throughput measurements. Reproduce by
extracting terminal into a standalone Rust program, iterating both formats and
None plus Some(0..=16), then comparing Vec len/capacity with
value.as_slice().to_vec().capacity(). That copy is only a capacity observation;
it allocates after formatting and is not a proposed optimization.

## Decision and alternatives

- Retain current construction unless representative failed-stream traffic shows
  sufficient benefit to justify additional prepared data or formatting code.
- Evaluate finite prepared record bytes with per-exchange owned Vec output, or
  exact-capacity construction through one canonical formatter. Compare static
  table size, cold preparation/storage, formatting work and copies. Do not build
  a new per-request cache or retain a writer across callbacks.

The potential benefit is removing repeated formatting and excess retained capacity
for nonzero terminal statuses only. A prepared record still needs the canonical
reservation-bearing output owner; Bytes::from_static alone is not equivalent.
Decide explicitly whether changed near-budget acceptance is intended. For example,
with 100 bytes of credit remaining, current nonzero terminal capacities exceed
credit while exact-sized records fit. This follows from capacity accounting; no
end-to-end acceptance experiment was run.

No table, formatter, output-owner API change or budget-policy change is authorized
by this task. This differs from [grpc-04](grpc-04-single-record-output.md), which
covers normal message aggregation inside a callback;
[grpc-07](grpc-07-terminal-message-header-storage.md), which covers percent-encoded
native/Web message headers; and [grpc-08](grpc-08-typed-terminal-status-evidence.md),
which covers original transport evidence across trailer companions.

## Acceptance before implementation

Prove byte parity for both formats and None/0 through 16, including delimiters,
fixed error text and absence of grpcStatus for missing evidence. Preserve strict
status validation, header/trailer conflict rules, error precedence, exactly-once
terminal publication, JSON length caps, cancellation and output failure handling.
Test exact and insufficient JSON caps and output credits, including live earlier
output, cloned output references and release only after the final reference.
Any changed budget acceptance needs an explicit reviewed expectation, not weakened
credit tests. Preserve the existing exchange reservation and streaming ownership;
add no response BodyStorage, full-response aggregation, tracing or payload logs.

Measure complete construction and handoff rather than only the copy step, using
representative successful/error/missing-status mixes and outstanding-output loads.
Report allocation count, retained capacity and CPU separately. Existing tests are
baseline evidence, not proof that a candidate is equivalent or worthwhile.
Consult the original gRPC admission/outcome ledger in
`skills/04-review/h2-grpc/regression-guards/grpc-json-transcoder-admission-and-outcomes.md`
and the capacity/lifetime tests in `budget_test.rs` before freezing an IMPL scope.
