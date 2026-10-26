# L4 listener read-buffer sizing

Status: deferred optimization decision for the user.
Source baseline: 7151f9d3f. No runtime, configuration or dependency change.

## Evidence and scope

This candidate covers plain TCP and raw TLS passthrough listeners, not HTTPS
termination or other listener products. Both use `runtime/server/listener_builder.rs::tcp_listeners`,
which calls Pingora `add_tcp` or `add_tcp_with_settings`. These construct a
`ListenerConfig::tcp` with default L4 buffer settings. No Gateway override of
`L4BufferSettings` was found in the current source.

The locked Edgion Pingora fork at `1aaa45cc70430bc782990bc20ddee3fef36d6b99`
constructs `protocols/l4/stream.rs::Stream` with a `(0, 0)` Tokio BufStream.
The accepted listener's `UninitializedStream::handshake` then calls `set_buffer`
before PROXY protocol handling and optional TLS handshake. Its defaults reserve
64 KiB for reads and 1460 bytes for writes. These are user-space buffer capacities,
not kernel socket buffers, resident-memory measurements or a total per-connection
heap bound.

The canonical Gateway `routes/l4_duplex.rs` relay separately owns two 8192-byte
buffers. Locked Tokio 1.53.1 `io/util/buf_reader.rs::poll_read` bypasses its
internal read buffer only when it is empty and the caller's remaining capacity
is at least the internal capacity. An ordinary 8192-byte relay read therefore
uses the listener's 65536-byte read buffer and copies selected bytes into the
relay buffer. Read-ahead can batch underlying reads; removing this copy may
increase system calls or change backpressure rather than improving throughput.

Built-in upstream TCP connections follow a different path: constructors retain
zero-capacity buffers, and connector `set_nodelay` changes TCP_NODELAY rather than
installing listener buffers. The relay does not flush upstream, while it flushes
downstream after each completed write. These facts do not establish missing
upstream flushing on the inspected built-in path, and do not cover custom or
injected transports. Do not conflate write-buffer policy with this read candidate.

## Choices and acceptance

Retain defaults until workload evidence selects a narrower listener policy, or
compare default, relay-sized and zero-capacity read buffers for these two listener
products. The existing fork listener configuration seam can express those sizes;
this task does not authorize adding a public configuration knob or modifying the
fork. Keep the 1460-byte downstream write buffer and flush contract independent.

Any implementation must select the buffer at listener initialization, before
PROXY protocol or application bytes are read. `set_buffer` extracts BufStream's
inner stream; applying it after reads or writes can discard buffered bytes. Do
not use it as a midstream optimization. Preserve PROXY protocol trust/optional
header handling, fragmented ClientHello inspection, all coalesced trailing bytes,
partial writes, directional fairness, read-based accounting, immediate EOF,
idle progress/deadlines and cancellation cleanup. Do not alter retained TLSRoute
termination code or HTTP/HTTPS listeners as incidental supporting work.

Measure small request/reply, bulk one-way and simultaneous two-way traffic,
segmented input, slow/stalled peers and long-lived mostly idle connections.
Record allocations/capacities, copy volume, read/syscall counts, CPU, throughput,
latency and resident memory separately. Exercise PP2 and TLS passthrough setup
with trailing bytes and backpressure under the canonical shared topology before
claiming equivalent byte delivery. Existing in-memory relay tests do not prove
transport buffering equivalence, and smaller capacity alone does not prove a
net performance gain. Preserve canonical logging; add no request tracing/metrics.

This differs from [relay-01](relay-01-idle-reset-coalescing.md), which concerns
timer reset scheduling, and [tls-04](tls-04-clienthello-buffer-capacity.md), which
concerns temporary ClientHello inspection storage rather than listener buffering.
Original authority: [shared relay contract](../../skills/04-review/routing/regression-guards/l4-duplex-relay-centralization.md).

Fresh unchanged-code verification passed 135 Gateway tests: 21 shared relay,
29 TCP and 85 TLS. Formatting and workspace all-target check/Clippy passed with
existing unrelated warnings. No candidate, benchmark, live integration, full
suite or fixture edit accompanies this decision.
