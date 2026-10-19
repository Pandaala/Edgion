# Evaluate retaining empty gzip output buffers

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Edgion baseline: `a886ea342`.
Inspected Pingora fork: `1aaa45cc70430bc782990bc20ddee3fef36d6b99`.

## Evidence and scope

Edgion's downstream ResponseCompression module can select the fork's gzip
Compressor for eligible known-length responses. In
`pingora-core/src/protocols/http/compression/gzip.rs`, Compressor::encode reserves
min(16 KiB, input length) in the encoder's output Vec, writes the input, optionally
finishes, updates statistics, then unconditionally returns
`mem::take(self.compress.get_mut()).into()`. This resets the encoder's output
buffer even when a call produced no bytes. The encoder need not emit data for
every input chunk; actual empty-output frequency remains to be measured.

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
for empty, tiny and large chunks; compressible and incompressible bodies; final
empty input; slow delivery; cancellation; and concurrent responses. Include the
current caller's returned-Bytes lifetime when comparing memory. Compare byte
streams and decompressed content, accounting and independent retained outputs;
verify exactly-once finishing and footer-before-trailer behavior through the
actual downstream module. Keep enabled/disabled, origin/cache and known-length
eligibility tests. No throughput or peak-memory improvement is established yet.
