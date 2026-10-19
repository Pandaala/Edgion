# gRPC integer normalization storage

Status: deferred user decision. Source baseline: `272aa3fd6`.

## Evidence and scope

The GrpcJsonTranscoder RequestFilter in
`edgion-gateway/src/plugins/http/grpc_json_transcoder/plugin.rs` calls the shared
Resources BoundRequest::encode after request capture and conversion admission.
`edgion-resources/src/grpc_transcoding/codec.rs` merges bindings and parsed body,
profiles the result, and invokes normalize before capped JSON serialization and
prost-reflect deserialization. This is request conversion, not response streaming.

In `edgion-resources/src/grpc_transcoding/normalize.rs`, integer validates string
inputs with exact_integer, handles JSON number inputs separately, and checks the
signed/unsigned 32/64-bit range. Every success replaces the value with
Value::String(n.to_string()), including an already canonical owned decimal string.
The replacement releases the old string after producing the new decimal text.
This is a potential repeated allocation, not a measured request bottleneck.

The previous binding optimization moves owned decoded strings into JSON, preserving
original spelling until this stage. Normalization still must turn leading plus,
leading zeroes, negative zero and accepted integral decimal/exponent spellings into
the same plain decimal output. Do not replace the exact scanner with floating-point
parsing or bypass validation merely because a value originated in a binding.
Body values, repeated/map values and exact canonical integer wrappers also reach
this normalization owner.

## Ownership tradeoff

Clearing and formatting into an existing String retains its capacity. A long
accepted zero mantissa or zero with a long exponent can normalize to one byte;
reusing its buffer keeps the old allocation alive. Even canonical-only reuse can
retain decoder/parser spare capacity, so checking text length alone is not a
capacity policy. Input and structural limits already bound these requests; this
observation does not establish an unbounded allocation or a current ledger breach.

The normalized JSON tree remains alive while CappedWriter allocates its fixed
MAX_JSON_BYTES buffer and serializes. codec explicitly drops the tree before
reflection. Evaluate retained capacity at this actual overlap, including earlier
normalized siblings and later input values, rather than adding unrelated request
and response peaks. The existing 32 MiB exchange reservation and process budget
must remain justified by the owning admission/outcome ledger; no automatic budget
increase is proposed.

## Alternatives for user decision

- Retain replacement and measure whether integer-heavy requests justify a new path.
- Reuse only already canonical strings under an explicit capacity criterion.
  Canonical detection must follow the existing parse/range checks and avoid
  allocating the comparison string merely to decide whether to avoid allocation.
  Measure comparison cost and the frequency of eligible body and binding values.
- Reformat validated numbers into eligible existing buffers, with fallback to
  replacement for large capacities. Select and justify the criterion; a new
  capacity threshold is a policy choice, not an inferred equivalence guarantee.
- Consider unconditional buffer reuse only with reviewed peak-memory accounting
  and evidence that retained capacity is acceptable. Shrinking afterward may
  reallocate and erase the expected benefit.

No implementation, threshold, new formatter or dependency is selected. Reuse a
suitable existing formatting mechanism if implementation is later authorized.

## Acceptance before implementation

Compare normalized JSON, errors and final protobuf wire values against the current
implementation for all signed/unsigned widths, min/max and adjacent overflow,
JSON integers/floats, positive/negative zero, leading plus/zeroes, exact fractional
and exponent spellings, malformed input and non-integral values. Include canonical
wrappers, nested messages, repeated/map fields and binding/body sources. Preserve
name/alias validation, null handling, structural charges and early failure order.

Exercise long zero and exponent strings near admitted input limits, short strings
with excess capacity, mixed small/large values and error paths. Measure allocations,
allocated bytes, retained capacity and peak live storage through serialization;
separate those results from throughput claims. Existing exact-scan and conversion
tests are baseline evidence, not a capacity or performance oracle.

Preserve canonical request BodyStorage, conversion admission/deadlines, fixed client
errors, redaction and access-log ownership. No response aggregation, request cache,
tracing or metric is needed. Re-evaluate if parser allocation behavior, input caps,
normalization order or the tree-to-reflection lifetime boundary changes.

Relevant original knowledge:
`skills/04-review/h2-grpc/regression-guards/grpc-json-transcoder-admission-and-outcomes.md`.
Fresh baseline verification is recorded in INDEX; no benchmark accompanies this task.
