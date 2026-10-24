# Evaluate retaining empty compression output buffers

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Initial gzip baseline: `a886ea342`. Brotli extension baseline: `4c76c48a0`.
Inspected Pingora fork: `1aaa45cc70430bc782990bc20ddee3fef36d6b99`.

## Evidence and scope

Edgion's downstream ResponseCompression module can select the fork's gzip
Compressor for eligible known-length responses. In
`pingora-core/src/protocols/http/compression/gzip.rs`, Compressor::encode reserves
min(16 KiB, input length) in the encoder's output Vec, writes the input, optionally
finishes, updates statistics, then unconditionally returns
`mem::take(self.compress.get_mut()).into()`. This resets the encoder's output
buffer even when a call produced no bytes. The encoder need not emit data for
every input chunk; deployment empty-output frequency remains to be measured.
The isolated gzip probe below establishes examples, not deployment frequency.

The same ownership pattern exists in the fork's
`pingora-core/src/protocols/http/compression/brotli.rs::Compressor::encode`:
reserve min(16 KiB, input length), write the input, flush only at end, update
statistics, then unconditionally take the output Vec. This is the compressor,
not the separately implemented Brotli decompressor. Gzip uses finish at end;
Brotli uses its existing writer flush. Neither terminal operation may be changed
as part of empty-output storage retention.

Brotli is reachable through `Algorithm::compressor`. The registered downstream
module initializes all algorithm levels from the selected level; Edgion's final
known-length/non-SSE eligibility gate applies before downstream encoding. This
source trace establishes reachability, not actual codec use or empty-output
frequency in a deployment.

The pinned bytes 1.12.1 implementation of From<Vec<u8>> retains spare capacity
through a separately allocated Shared owner when length differs from capacity.
Thus an empty Vec with capacity transfers its allocation to the returned Bytes;
it is not necessarily freed at conversion. The fork comment claiming that this
conversion drops excess capacity does not describe this dependency implementation.
The next encoder call starts with an empty zero-capacity Vec regardless of when
the previous Bytes is dropped. These are source observations, not allocator or
end-to-end benchmark results.

Evaluate keeping an empty output Vec in the encoder and returning empty Bytes,
while retaining the current ownership transfer for nonempty output. This could
avoid repeated reserves and empty Shared owners, but holds capacity in the
encoder longer. The 16 KiB reserve argument is not a hard capacity bound: Vec
growth and encoded output may exceed it. This is separate from compression-01's
Accept-Encoding preference storage and context-01's outer module storage.

## Boundaries and decision

Keep this deferred until the user selects the memory/CPU tradeoff and authorizes
an exact external Pingora-fork scope. Do not edit Cargo's dependency checkout or
change the workspace's dependency revision as incidental cleanup. This task
grants no external mutation authority.

Preserve compressed bytes and their order, Some(empty Bytes) versus None,
input/output accounting, encoder errors, finalization and cancellation cleanup.
Duration measurements naturally vary with executed work; retain their existing
measurement scope. Do not reuse storage still owned by nonempty returned Bytes.
Do not add pooling, response aggregation, decompressor changes, extra flushes,
new logging/metrics, negotiation changes, or compression policy changes.
Edgion's captured config and known-length/non-SSE gate remain mandatory; see
`skills/04-review/http1/compatibility-constraints/compression-requires-known-response-length.md`.
Preserve terminal/footer ordering described in
`skills/04-review/h2-grpc/regression-guards/trailer-done-terminal-body-dispatch.md`,
revalidating the current fork rather than treating historical tests as fresh proof.

## Acceptance for a later implementation

Measure empty-output frequency, allocation calls/bytes, live capacity and CPU
separately for gzip and Brotli, using empty, tiny and large chunks; compressible
and incompressible bodies; final
empty input; slow delivery; cancellation; and concurrent responses. Include the
current caller's returned-Bytes lifetime when comparing memory. Compare byte
streams and decompressed content, accounting and independent retained outputs;
verify exactly-once finishing and footer-before-trailer behavior through the
actual downstream module. Keep enabled/disabled, origin/cache and known-length
eligibility tests. No throughput or peak-memory improvement is established yet.

The earlier Brotli extension reused this decision rather than creating a second task.
No fork code, dependency revision, runtime tests or benchmarks were changed or
run for that documentation-only extension.

## Isolated gzip output probe

At Edgion `701c15579`, a temporary standalone Rust probe linked the workspace's
existing flate2 1.1.9 artifact, confirmed against Cargo.lock and its dependency
file. It reproduced the locked fork compressor's level-6 sequence: reserve
min(16 KiB, input length), write_all, try_finish on the final call, then take the
output Vec. It did not invoke the fork module, convert Vec to Bytes, measure
allocations or duration, or implement the proposed retention branch.

Each case used 65,536 input bytes, split into the chunk size below, followed by
one separate empty terminal call. Repeat input is ASCII `a`. Xorshift input starts
with u32 state `0x12345678`; for each byte apply XOR-left-13, XOR-right-17 and
XOR-left-5 in that order, then emit the low byte. This is a deterministic fixture,
not an entropy or worst-case compression claim. Concatenated outputs were decoded
with GzDecoder and asserted equal to the complete original input in every case.

| Chunk bytes | Input | Calls including EOS | Empty outputs | Empty outputs with capacity | Sum of empty Vec capacities |
| --- | --- | --- | --- | --- | --- |
| 1 | Repeat | 65,537 | 65,535 | 65,535 | 524,280 |
| 1 | Xorshift | 65,537 | 65,532 | 65,532 | 524,256 |
| 64 | Repeat | 1,025 | 1,023 | 1,023 | 65,472 |
| 64 | Xorshift | 1,025 | 1,020 | 1,020 | 65,280 |
| 1,024 | Repeat | 65 | 63 | 63 | 64,512 |
| 1,024 | Xorshift | 65 | 60 | 60 | 61,440 |
| 16,384 | Repeat | 5 | 3 | 3 | 49,152 |
| 16,384 | Xorshift | 5 | 1 | 1 | 16,384 |

Capacity sums are bytes summed across successive empty outputs, not simultaneous
live memory, allocation counts/bytes, or estimated savings. The probe immediately
appended output to a collector and dropped each output Vec; it did not model
downstream Bytes retention, backpressure, cancellation or concurrency. Compressed
lengths were 97 bytes for Repeat and 65,576 for Xorshift for every chunk size.
Two coordinator runs and an independent plan-review rerun reproduced the table.
The current gzip sequence can therefore produce empty allocated output; the
memory/CPU tradeoff and external-fork authorization remain deferred. Brotli was
not exercised by this probe.

Formatting and workspace all-target check/Clippy passed with existing warnings.
No full suite, live compressed-wire exchange or throughput benchmark ran.
