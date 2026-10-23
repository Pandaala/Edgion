# ExtProc codec buffer policy

Status: deferred performance decision for the user. Baseline: `dfe157e6d`.
No runtime change or selected buffer size.

## Evidence

Gateway ExtProcClient uses Common's generated ExternalProcessorClient. The
generated Process method creates ProstCodec::default and calls inner.streaming
after readiness, static RPC path and GrpcMethod metadata setup. The generator is
`edgion-common/build.rs`; do not edit generated OUT_DIR files. This client uses
workspace tonic 0.12.3, not the separately declared OTLP tonic 0.14 dependency.

In the locked tonic source, codec/mod.rs defaults BufferSettings to an 8 KiB
buffer and a separate 32 KiB encoding yield threshold. codec/encode.rs allocates
the initial BytesMut when constructing EncodedBytes; additional uncompressed
scratch is allocated only when compression is enabled. codec/decode.rs creates
Streaming with its own initial BytesMut and empty decompression scratch. The
decoder checks the announced message length against the message limit before
reserving that length. Buffer capacity is not a message-size limit.

These allocations occur at different RPC stages, not at plugin construction or
all together when ExtProcStream's lazy opening future is created. Encoded output
uses split_to(...).freeze(), so queued Bytes can own backing storage independently
of the active encoder buffer. Decoder storage, growth/reclamation and protobuf
payload storage also have distinct lifetimes. Two initial capacity requests are
not evidence of exactly 16 KiB simultaneous heap, fixed savings, or RSS usage.

## Decision

Retain the codec defaults, or investigate an ExtProc-specific codec policy with
measured initial capacities. Smaller initial buffers may reduce small-message
RPC allocation requests but add growth/copying for larger messages. Header-only,
streamed-body and buffered-request modes have different message distributions.
Neither a new size nor net performance value is established.

ProstCodec exposes raw_encoder/raw_decoder constructors with BufferSettings, but
the generated method selects its own default codec. An implementation therefore
needs a reviewed generated-client/custom-codec seam, not just a channel setting.
Enumerate affected consumers and freeze an exact allowlist before choosing that
seam. Do not fork unrelated generated behavior or change configuration-sync,
OTLP or global tonic defaults as a shortcut.

## Acceptance for a selected implementation

- Preserve protobuf bytes, message boundaries, readiness, RPC path/metadata,
  compression negotiation, existing message-size limits and rejection ordering.
  Keep the 32 KiB yield threshold unless a separate buffering/backpressure review
  explicitly approves changing it. Initial capacity is not permission to relax
  any request, response-window, mutation or local-reply bound.
- Preserve lazy stream opening, phase-matched verdicts, observable unsolicited
  verdict checks, partial-frame state across Pending, per-message timeout, clean
  half-close/terminal drain, cancellation, permit release and failure policy.
  Dropping or reopening streams to reclaim buffers is outside this proposal.
- Measure allocations/requested bytes, actual capacity/backing ownership, peak
  overlap, CPU and end-to-end exchange latency. Cover tiny/default/large headers,
  request/response body windows, fragmentation, many concurrent admitted streams,
  sequential small/large/small messages and cancellation. Separate first exchange,
  steady state, outbound Bytes retention and final destruction. No buffer-count
  arithmetic alone can establish process memory savings.
- Retain the network-free real-Tonic partial-frame regression and existing
  phase/timeout tests. Validate any codec seam against the current generated
  client and actual gRPC transport before claiming interoperability. Require
  independent IMPL plan/final review and project checks.
- No new buffer pool, cache, request tracing, metrics or protobuf field mapping.
  [body-02](body-02-extproc-protobuf-body-storage.md) covers HttpBody representation;
  [extproc-01](extproc-01-terminal-stream-retirement.md) covers destruction order.
  Neither task authorizes codec policy changes.

Fresh baseline verification passed 30 ExtProc client tests. No codec prototype,
allocation/latency benchmark or live transport experiment was run. The existing
[interactive exchange](../../skills/04-review/ext-proc/regression-guards/ext-proc-interactive-continue-and-replace.md)
and [phase verdict](../../skills/04-review/ext-proc/regression-guards/ext-proc-phase-verdict-matching.md)
contracts remain authoritative.
