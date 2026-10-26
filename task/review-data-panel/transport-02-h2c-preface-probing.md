# Prefix-aware h2c connection probing

Status: deferred behavior and optimization decision for the user.
Source baseline: f6fcde124. No runtime or dependency change.

## Reachability and current behavior

Gateway `runtime/server/listener_builder.rs::add_http_listener` enables
`HttpServerOptions.h2c` for plaintext HTTP when HTTP/2 is enabled. Ordinary proxy
construction leaves `force_custom` false. In the locked Edgion Pingora fork
`1aaa45cc70430bc782990bc20ddee3fef36d6b99`, `apps/mod.rs::process_new` disables
h2c probing for TLS streams and otherwise probes when h2c is enabled and custom
mode is false. TLS protocol selection remains ALPN-owned.

The probe asks `try_peek` for the complete 24-byte H2 preface before comparing
its contents. The concrete L4 Stream implementation reads until that length is
available or a read returns EOF/error, then rewinds consumed bytes. It does not
stop when an early byte disproves the preface. For example, the 18-byte input
`GET / HTTP/1.0\r\n\r\n` cannot complete that probe while the peer stays open
without more bytes, even though its first byte is not P. This is a source-level
inference, not a reproduced Gateway integration result or production report.

A successful peek compares all bytes: mismatch falls through to HTTP/1. EOF or
returned I/O error restores consumed bytes within try_peek, but process_new's
error-to-None path then closes this connection rather than trying HTTP/1. An
unsupported Peek returning Ok(false) leaves the h2c flag enabled. These distinct
outcomes must not be silently folded into one fallback rule.

The listener service's 60-second timeout encloses only `io.handshake()`.
`handle_event` and application protocol detection execute after that future
returns. HTTP/1 keepalive setup and H2 session policies are later still. The
inspected call chain supplies no equivalent timeout around this probe; this is
not a measured unlimited connection lifetime or a claim about every outer owner.
Rewind on returned outcomes does not prove cancellation-safe replay when a pending
future is dropped. Normal HTTP/1 reuse runs inside the HTTP/1 processing loop
without repeating initial protocol detection.

## Decision and acceptance

Consider a fork-owned prefix-aware probe that decides HTTP/1 on the first
mismatch, while preserving consumed bytes and waiting for a complete matching H2
preface. It could reduce setup waiting for short or segmented non-H2 traffic.
This changes when a connection reaches HTTP/1 and which errors are encountered;
it is not an automatically behavior-preserving copy optimization. No throughput,
allocation, latency or security improvement is measured. Probe payload is only
24 bytes, not an unbounded request-body copy. Keep the current implementation
until the user selects a behavior contract and separately authorizes fork work.

Specify short EOF, read failure, exact H2, matching partial prefixes, early/late
mismatch, unsupported Peek and custom-transport outcomes before implementation.
A new probe deadline is a separate policy decision; do not repurpose HTTP/1
keepalive, H2 stream/idle or transport-handshake timeouts. Preserve TLS/ALPN,
first-request versus HTTP/1 reuse, startup protocol flags, stream ownership,
shutdown/cancellation cleanup, and every consumed byte exactly once. Avoid
Gateway-local duplicate protocol detection or new request tracing/metrics.

Verification must exercise the actual fork application entry with segmented
in-memory transport and explicit pending/EOF/error states, then the canonical
shared Gateway topology for short H1, ordinary H1, h2c and TLS/ALPN. Include
valid prefix plus trailing H2 bytes and non-H2 payload replay. Measure setup
latency and read/copy work separately; successful listener-builder tests alone
cannot establish wire behavior. Keep this separate from inbound PROXY's bounded
12-byte optional probe and from relay-02 read-buffer sizing.

Original policy: [downstream H2 startup limits](../../skills/04-review/h2-grpc/regression-guards/downstream-h2-startup-policy.md)
and [HTTPS ALPN ownership](../../skills/04-review/tls/regression-guards/https-http2-alpn-policy-handoff.md).
Fresh unchanged-code Gateway downstream-H2/listener-builder selection passed
eight tests. Formatting and workspace all-target check/Clippy passed with
existing unrelated warnings. No candidate, fork tests, live transport, full
suite or fixture change was included.
