# TLS passthrough ClientHello buffer capacity

Status: deferred initialization and retained-capacity decision for the user.
Source baseline: 5b52f2d90. No runtime change.

## Evidence and current boundary

EdgionTlsPassthroughProxy::process_new in routes/tls/passthrough_proxy.rs creates
vec![0u8; MAX_CLIENT_HELLO_LEN] after its shutdown check. The configured constant
in sni_parser.rs is 16,384 bytes including the five-byte TLS record header; it
does not admit a full 16 KiB fragment plus that header. The buffer begins with
that initialized logical length before any ClientHello read.

read_client_hello reads into the entire remaining slice until a complete first
record is available, the absolute 15-second deadline expires, the connection
fails, or the existing cap/non-handshake rules reject it. SNI extraction receives
only record_end bytes. The caller truncates the Vec to filled, preserving every
byte already drained, including any coalesced trailing data.

Truncation does not reduce capacity. The allocation moves into handle_connection
and can remain during route/plugin work, upstream selection/connect retries and
optional PP2 writing. replay_initial_bytes then writes and flushes all filled
bytes, accounts for them only after success, and explicitly drops the Vec before
long-lived relay. Early exits also drop it. This is a setup-retention opportunity,
not a claim that every established connection keeps a 16 KiB inspection buffer.
The existing relay has its own separate bounded buffers.

## Candidate choices and tradeoffs

- Keep the fixed initialized buffer until connection-rate, input-size and setup
  concurrency measurements justify changing it. This defers optimization; it
  does not close the broader performance review.
- Start with a smaller initialized buffer and grow only as needed, under the
  same total cap and absolute deadline. Inspecting the declared record length
  after five bytes may guide bounded growth, but length remains lower-trust
  input. Growth adds allocations/copies and changes read sizes/counts.
- Compact the filled prefix after inspection if measured setup retention
  warrants the copy or allocator operation. This does not remove the initial
  full-size allocation/initialization, and shrink_to_fit is not a guarantee of
  returning pages to the OS. Avoid unconditional compaction without evidence.

Do not infer realistic ClientHello sizes from an old typical-size comment:
measure current TLS versions, extension sets and post-quantum handshakes. Report
requested/retained allocation capacity separately from RSS, syscall count,
latency and throughput. A global reusable pool would retain memory and introduce
cross-connection ownership/content concerns; it is not authorized by this task.
Do not replace initialized reads with unsafe uninitialized slices or manually
set Vec length without a separately reviewed safe read contract.

## Behavioral acceptance

Smaller reads can consume less trailing data before connecting upstream. This
need not lose bytes, but it changes which bytes belong to initial replay versus
later relay. Explicitly review segmentation, scheduling and accounting instead
of assuming identical behavior from an equal final byte stream alone.

Preserve complete-record gating, the same total inspection cap and first-byte
rejection, original malformed-container checks and first-SNI selection. Keep one
absolute deadline across all reads/growth operations; never reset it on progress.
Retain timeout, disconnect, oversize and malformed-record diagnostics and no
upstream attempt on failed SNI parsing. Do not add support for fragmented TLS
handshake messages or change parser acceptance as part of buffer tuning.

Forward every drained byte exactly once in order, including 0-RTT/trailing
records, then continue with unread downstream bytes in order. Never truncate to
record_end for replay. Preserve optional PP2-before-replay ordering, initial
write-plus-flush completion, error behavior and successful-byte accounting.
Release inspection storage on rejection, cancellation, connection failure and
successful replay before relay. Preserve selected-route generation, retry and
outlier semantics, and existing setup timeout policy.

Tests must cover one-byte/fragmented input, complete records with coalesced tails,
near-cap and over-cap records, slow dribble, disconnect, malformed containers,
short writes, flush failure and cancellation. Revalidate actual socket-level
ClientHello fixtures for any selected implementation; current unit baselines do
not establish new read-batching equivalence or live performance.

Original knowledge: [ClientHello read contract](../../skills/04-review/tls/regression-guards/sni-passthrough-clienthello-read.md)
and [passthrough ownership](../../skills/04-review/routing/regression-guards/tls-passthrough-connection-ownership.md).
