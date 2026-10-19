# Prepared gRPC wire-admission schema

Status: deferred user decision. Source baseline: `dcae58f05`.

## Evidence and scope

Gateway response conversion in
`edgion-gateway/src/plugins/http/grpc_json_transcoder/response.rs` sends unary
payloads and completed streaming messages through CompiledBinding::decode.
`edgion-resources/src/grpc_transcoding/codec.rs` checks response bytes, runs the
canonical wire preflight, then constructs DynamicMessage and writes bounded JSON.
The borrowed ProtoJson serializer already forwards recursively without building
another JSON tree; its collect_str forwards to the capped serializer.

In `edgion-resources/src/grpc_transcoding/wire.rs`, walk resolves every field tag
through MessageDescriptor::get_field, obtains Kind and is_list, and uses that shape
for nested messages, packed scalar work and expected wire type. Locked prost-reflect
0.14.7 already stores field_numbers in a BTreeMap. A successful lookup constructs
an owned FieldDescriptor by cloning its Arc-backed message handle. Kind can contain
message/enum handles; is_list can inspect kind again for repeated fields. This is
metadata lookup and handle work, not descriptor-tree copying or proof of a heap
allocation on every tag. No measured bottleneck has been established.

The same walker also serves cold descriptor admission through walk::<true>.
That mode resolves trusted extensions when ordinary lookup misses, rejects unknown
fields, and rejects field 999 on Options messages. Runtime walk::<false> does not
perform that extension fallback and permits unknown fields with supported wire
encodings. Unknown length-delimited data stays opaque; it must not be recursively
interpreted as a message merely because its contents resemble one.

## Ordering and bounds to preserve

The walker checks depth and charges a message node before visiting fields. Each
field charges a node before parsing its tag. Packed elements charge individually;
known nested messages recurse under the same budget. Payload consumption precedes
the final known-field expected-wire check. Preserve these decisions and their early
failure work, including truncated/overlong varints, invalid field numbers, malformed
lengths, unsupported groups/wire types, packed widths and unknown fields.

Current compilation bounds the descriptor inventory at 1024 messages and 8192
ordinary fields, with separate byte, namespace and structural admission before
reflection. These bounds are not proof that any duplicated representation fits a
chosen cold-memory budget. Legal field numbers reach 0x1fffffff, so allocating a
dense array indexed by the maximum tag is inappropriate. Recursive references and
shared subgraphs require deduplication by schema identity; recursively expanding
them into trees or creating owning Arc cycles would change memory behavior.

## Alternatives for user decision

- Keep the existing indexed descriptor path and measure realistic response traffic,
  including small messages, repeated scalars, nested messages and unknown fields.
- Investigate a compact immutable field-shape graph prepared per compiled generation.
  Compare sorted sparse tables and other bounded indexes with the dependency's
  existing lookup. Use bounded node identities for recursive/shared references and
  account for construction scratch, retained bytes and hot-reload overlap.
- Retain the canonical wire walker and define a narrow shape-provider boundary only
  if measurement justifies it. Runtime specialization must not silently weaken the
  descriptor-admission mode or introduce a second drifting wire parser. Any change
  to the descriptor path requires its own validation evidence.

No implementation, table layout, cache or new dependency is selected. A process-wide
mutable descriptor cache or lazy request-time graph construction is not proposed.
The exchange reservation, process conversion budget and response streaming ownership
remain unchanged; generation-owned prepared data needs separate reviewed accounting.

## Acceptance before implementation

Differentially compare acceptance and rejection against the current walker for
valid and malformed wire, unknown fields, sparse/high legal tags, packed/unpacked
forms, all scalar widths, nested/map messages, repeated occurrences and exact
node/depth/byte boundaries. Include recursive schemas with finite messages and
shared subgraphs. Exercise both standalone preflight and actual JSON conversion.
Descriptor-mode regressions must preserve trusted extensions, unknown rejection,
uninterpreted Options field 999 rejection and pre-reflection limits.

Measure CPU, handle operations, cold build time, construction peak and retained
bytes per generation, including reload with old requests still alive. Verify
cancellation and generation retirement release prepared state. Existing 59 targeted
tests are baseline evidence, not proof of a new graph's equivalence or performance.

Preserve fixed client failures, redaction, canonical access-log ownership, response
caps and backpressure. Add no request tracing, metrics or full-response aggregation.
Re-evaluate when prost-reflect indexing, schema admission, wire policy or generation
ownership changes. Relevant original knowledge is
`skills/04-review/h2-grpc/regression-guards/grpc-json-transcoder-admission-and-outcomes.md`.
