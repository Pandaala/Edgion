# Outbound Content-Length integer storage

Status: deferred for user decision. No runtime change.
Source baseline: 8ea72ea84.

## Evidence and scope

The canonical `sync_outbound_body_framing` helper in
`edgion-gateway/src/plugins/http/common/outbound/request.rs` formats the final
buffered body length into a temporary String, then copies and validates its
decimal bytes through `HeaderValue::from_str`. Shared outbound request assembly,
post-override finalization and Wasm callout construction use this helper.

Locked `http` 1.5.0 supports `HeaderValue::from(usize)`. Its `from_integers!`
implementation formats through a stack `itoa::Buffer` into a BytesMut reserved
for the maximum decimal width, then freezes that storage. The requested capacity
is 20 bytes on 64-bit targets and 10 on 32-bit targets. The existing `from_str`
path copies only the actual decimal bytes into final header storage. Integer
conversion would eliminate the temporary String and validation, but retain more
requested capacity for short lengths. Locked `bytes` 1.12.1 also routes this
freeze through `Bytes::from(Vec<u8>)`, which allocates shared metadata when
length differs from capacity. Eliminating the temporary String therefore does
not prove fewer total allocation calls. Neither lower peak memory nor lower
physical allocator usage is established; allocator size classes and storage
metadata also matter. No throughput or allocation benchmark has been run for
this candidate.

Decimal usize text is always valid header text. The helper and its private
`finalize_request` wrapper could become infallible, with unreachable error
propagation removed from callers. The reason for deferral is the storage
tradeoff, not a demonstrated wire-format or error-policy regression.

## Decision requested

Retain the current implementation, authorize the integer conversion with its
capacity tradeoff, or investigate another conversion that avoids temporary
storage without increasing retained capacity. Keep framing in the canonical
helper; do not introduce a separate formatter or dependency without evidence.

## Acceptance before implementation

Use IMPL with a frozen source allow-list covering the helper and its actual
callers. Preserve the order of stripping stale Content-Length/Transfer-Encoding,
the final body as the length source, absence of framing for no byte body, explicit
zero for an empty byte body, and the nonsensitive header flag. Retain the current
handling of non-byte bodies; this does not authorize streaming-body support.

Verify absent, empty and multiple-digit body lengths, caller body overrides,
extra-header precedence and Wasm request construction. Use shared outbound tests
and the two pure Wasm request-builder cases in
`edgion-gateway/tests/unit/plugins/http/wasm/callout_test.rs`; the broader Wasm
callout selection contains network tests and is not a network-free unit gate.
Any investigation of those existing tests is separate scope.

Measure allocation calls and requested bytes separately from retained storage,
peak lifetime and physical allocator usage. Include short and long decimal
lengths and state target width. Run formatting, workspace check/Clippy and
applicable repository guards. Do not infer an overall memory or throughput
improvement from removing one temporary allocation.
