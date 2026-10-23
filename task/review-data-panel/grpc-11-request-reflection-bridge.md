# Evaluate the gRPC request reflection bridge

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `0d13ddfc8`.

## Evidence and decision

Gateway GrpcJsonTranscoder calls Resources BoundRequest::encode after conversion
admission. In `edgion-resources/src/grpc_transcoding/codec.rs`, encode parses the
bounded body, merges path/query fields, profiles the combined JSON tree and runs
schema-aware normalization. It then serializes the normalized Value through a
fixed-capacity 1 MiB CappedWriter, drops that tree, compacts the serialized buffer,
and reparses it through DynamicMessage::deserialize. After the deserializer end
check it drops parsing scratch and serialized input before allocating the framed
protobuf output. This bridge is request conversion, not response streaming.

Locked prost-reflect 0.14.7 accepts a generic Serde Deserializer. Locked
serde_json 1.0.151 implements Deserializer for both Value and &Value; owned
strings can reach visit_string. This makes a tree-backed reflection path a
candidate for investigation, not proof that dependency visitors consume every
allocation by move or accept exactly the same input as the textual bridge.

Decide whether bypassing normalized text storage and reparsing is worthwhile.
The existing implementation has explicit lifetime and admission boundaries.
Direct owned-tree consumption may gradually move/drop values while building the
reflected message; borrowed-tree consumption retains the entire source tree.
Neither necessarily reproduces the present peak, which drops the JSON tree
before reflection. No implementation, performance winner or new dependency is
selected. Retain the current bridge unless measured benefit justifies a reviewed
replacement contract.

## Admission and ownership constraints

Keep parsing, merge, profile and schema-aware normalization in their existing
order. The normalized serialized-byte cap is independent of profile text bytes:
JSON escaping, keys and punctuation contribute to encoded size, and normalization
can change spelling. Removing CappedWriter without an equivalent gate weakens
admission. A capped counting writer before reflection could preserve the gate
without retaining text, but still performs serialization work. Any alternative
must prove the same cap and rejection ordering before reflection allocations.

The [conversion ledger](../../skills/04-review/h2-grpc/regression-guards/grpc-json-transcoder-admission-and-outcomes.md#memory-and-time-ledger)
reserves 32 MiB per exchange under the process-wide budget. It explicitly releases
the JSON tree before reflection. Recalculate actual overlap for a candidate:
source body, merged/normalized trees, unconsumed containers and spare capacity,
reflected fields, WKT typed trees/transcode buffers and final frame. Moving String
values is not proof that all containers or WKT intermediates disappear. Keep
node/depth/schema admission before dependency allocations and justify every new
peak within the reservation; no automatic budget increase is proposed.

Preserve protobuf and JSON aliases, duplicate/overlap errors, unknown fields,
null/default handling, integer ranges, float behavior, bytes/enums, maps/lists
and supported well-known types. Generic Deserializer support is not parity
proof for visitor selection or numeric conversion. The current text end check
must not be casually replaced by accepting partially consumed input. Successful
encoding must own its output after every input/tree/binding temporary is dropped.
Keep encoded-size rejection before frame allocation, frame prefix/length, error
dialect, unwind isolation and reservation/cancellation behavior unchanged.

## Acceptance for an approved investigation

Compare full encode allocation calls/bytes, CPU, live/peak retained heap and
admission failures for small, near-cap, deeply nested and wide requests. Include
path/query/body merges, escaped text and keys, normalized numeric inputs, WKT
wrappers and structured WKT values. Count retained capacities and reflection
scratch, not only logical payload length. A microbenchmark of Value deserialization
alone does not establish complete request improvement.

Differential tests must compare acceptance, error category and successful framed
protobuf semantics across valid/invalid inputs, cap boundaries, alias collisions,
unknown fields, nulls, scalar ranges, malformed body JSON and overlapping failures.
Exercise lifetime independence and cancellation at the Gateway owner. Preserve
existing source tests and canonical JSON/schema mechanisms; do not fork a parser
or bypass required validation. Require independent IMPL design/final review and
applicable project checks before any runtime change.

This is separate from [grpc-02](grpc-02-integer-normalization-storage.md) numeric
buffer reuse, [grpc-09](grpc-09-wkt-output-transcoding.md) response WKT output, and
[grpc-10](grpc-10-mapping-selector-storage.md) binding selector containers.
Fresh existing tests passed 35 Resources grpc_transcoding and 31 Gateway transcoder
cases (66 selected executions). No candidate implementation, allocation/CPU
benchmark or live gRPC integration ran.
