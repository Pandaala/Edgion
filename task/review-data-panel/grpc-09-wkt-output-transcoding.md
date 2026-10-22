# Evaluate well-known-type ProtoJSON output intermediates

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `cfd20fee3`; locked prost-reflect: `0.14.7`.

## Evidence and scope

Gateway unary and streaming conversion reach Resources
`grpc_transcoding/codec.rs::CompiledBinding::decode_inner`: byte and wire admission,
owned DynamicMessage decoding, then bounded serialization through the borrowed
`serialize.rs::ProtoJson` adapter. Ordinary reflected fields are serialized by
reference; the adapter does not construct a second JSON tree.

The dependency's `dynamic/serde/ser/mod.rs` dispatches exact WKT names through
`dynamic/serde/ser/wkt.rs`. Many of those serializers call
`dynamic/mod.rs::DynamicMessage::transcode_to`, which encodes a temporary protobuf
Vec and decodes a typed value before JSON emission. This covers the numeric,
boolean, string and bytes wrappers, Timestamp, Duration, FieldMask, Struct,
ListValue and Value. String/bytes wrappers can additionally copy their content;
the structured types construct typed subtrees. Empty/default encoded buffers need
not allocate. This is not one allocation for every field or every nested node:
after a Struct/ListValue/Value transcode, its typed descendants serialize
recursively without each repeating DynamicMessage transcoding.

Empty has its own direct serializer. The dependency also has an Any serializer,
but Edgion rejects Any and HttpBody descriptors; those paths are not an admitted
optimization target. Edgion validates canonical shapes for the transcode-backed
WKTs listed above.

The generic ProtoJson serializer adapter receives scalar/container callbacks after
the dependency has selected and performed the typed conversion. Optimizing that
adapter alone cannot remove the earlier transcode. The existing conversion memory
ledger explicitly budgets these temporary encoded buffers and typed trees. This
investigation establishes an implementation cost, not a budget defect or measured
production bottleneck.

## Decision and alternatives

- Retain the current dependency behavior and measure its actual contribution.
- Investigate an upstream improvement or reviewed dependency change that removes
  redundant conversion while preserving its ProtoJSON contract.
- Consider a narrowly scoped borrowed WKT output implementation only if measurements
  justify taking ownership of those semantics. It must fit the canonical Resources
  codec; a Gateway-local second serializer is not an acceptable shortcut.

No fork, dependency upgrade, replacement serializer or new cache is authorized by
this task. Measure ordinary-message controls and WKT-heavy root/nested/repeated
cases, empty/default values, escaped text, bytes and large structured values.
Separate temporary encoded bytes, typed-tree allocations, retained capacities,
peak overlap and CPU from final output storage. Include complete callback costs
and failure cases; lower allocation counts alone do not prove a worthwhile change.

## Acceptance boundary

Preserve exact output bytes and errors, default/presence/omission behavior, field
ordering, quoted 64-bit integers, base64, Timestamp/Duration validation and
formatting, FieldMask transformations, wrapper nonfinite strings and Value's
nonfinite rejection. Verify canonical-name and schema-shape handling, unknown
fields and nested collection semantics against the locked implementation.

Keep byte/depth/node admission before reflection, panic containment, bounded JSON
including NDJSON/SSE framing, independent callback/output credits, real terminal
status, cancellation cleanup and reservation-bearing output ownership. Any changed
intermediate representation needs a reviewed peak-memory ledger; do not lower
existing reservations merely because one buffer disappears. Add no response
BodyStorage, unbounded aggregation, payload diagnostics or hot-path tracing.

This differs from grpc-03's prepared field-shape graph, grpc-04's callback output
handoff and grpc-02's request integer normalization. Their deferred decisions are
unchanged. Original authority:
`skills/04-review/h2-grpc/regression-guards/grpc-json-transcoder-admission-and-outcomes.md`.

Fresh unchanged-code verification passed 35 Resources and 29 Gateway transcoder
tests (64 total), formatting, workspace all-target check/Clippy and agent-doc
validation. Existing unrelated warnings remain. These checks do not validate a
replacement serializer. No prototype, new tests, full suite, live integration or
performance benchmark ran.
