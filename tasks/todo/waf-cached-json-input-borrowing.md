# Avoid token copies for contiguous cached JSON input

Status: Conditional optimization; P2

BodySnapshotReader currently splits resident data into bounded chunks. A long unescaped JSON token can therefore enter carry storage even when one immutable resident source is available.

- Measure this cost first; the default spill threshold limits the eligible workloads.
- Consider borrowing a non-materializing slice only for a proven contiguous, resident immutable snapshot with a retained owner and bounded admission.
- Preserve actual EOF, reader parity, cancellation, and edited-body semantics. Live, multi-segment, and spilled sources keep bounded chunk processing.
- Do not replace this with unconditional read_all or another request-body cache.
- Verify long tokens, malformed/truncated input, exact limits, cached/edited/live/spilled sources, and allocation/copy savings.

Start from `edgion-gateway/src/request_body/storage.rs` and `edgion-waf/src/seclang/json_stream.rs`.
