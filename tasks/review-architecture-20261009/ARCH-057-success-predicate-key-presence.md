---
id: ARCH-057
status: candidate
kind: behavior-defect
priority: P2
partition: PART-205
analyst: /root
baseline: BASELINE-AUDIT-20261010-C.json
---

# Preserve operator key presence when decoding outbound success predicates

`BodyPredicate` owns the exactly-one-operator admission contract for outbound
success classification. Its private `RawBodyPredicate` in
`edgion-resources/src/resources/common/success.rs:43-57` represents each operator
as `Option<T>`, and `BodyPredicate::deserialize` at lines 66-101 counts only
non-None values. Serde treats an explicit null as None, just like a missing field.
For JSON equality operands, null is a legitimate `serde_json::Value` whose
identity must survive decoding; the runtime comparator and derived serializer
already support `PredicateOp::Equals(Value::Null)`.

The documented strict contract is not enforced for explicit-null keys. The
input `{"pointer":"/error","equals":null}` is rejected as zero operators.
`{"pointer":"/error","equals":null,"exists":true}` instead counts one and
becomes only `Exists(true)`. On a JSON response `{"error":"denied"}` with an
accepted status, that surviving presence check succeeds. This is a static
counterexample to the exactly-one-operator invariant, not a claim that a deployed
auth service suffered a bypass. Derived serialization of an explicitly
constructed Equals(Null) predicate also produces a document the same type
cannot deserialize, affecting round-trip configuration transport.

The consequence has real owners: `ExternalHttpConnection.success` supplies this
type; `validate_connection_target` calls `validate_success` only after decoding
has discarded the extra null key. `ManagedOutboundClient::classify_detail`
delegates to this canonical classifier. ForwardAuth's
`handle_outbound_response` lets Success continue to the upstream and turns
BodyFail into 403. The bounded outbound retry loop also consumes BodyFail for
`retryOnBodyFailure`. The classifier is intentionally shared; a consumer-specific
patch would leave the faulty admission contract elsewhere.

Distinguish missing operator keys from supplied values in this private admission
representation. A bounded field deserializer can use the existing default None
for absence while deserializing a present equality operand directly as Value
and wrapping it in Some, including Null. Count supplied keys, then construct
the one typed operator. Present null for bool/list operators must either retain
presence until typed rejection or fail decoding directly; it must not disappear
next to another valid operator. Preserve the current unknown-field and duplicate
field rejection. Do not switch to a flattened externally tagged enum that takes
the first match, or weaken exactly-one validation.

An explicit small map visitor is an alternative if it is simpler to prove key
presence and typed operands together. Keep it local to BodyPredicate, with no
general configuration framework or changes to the public operator shape.
Cold admission is the right owner; request classification need not change or
allocate more. Preserve strict JSON comparison, missing-pointer failure for
equals/notEquals/in, legitimate exists:false, status/body separation, all-body
AND semantics, empty-status/in rejection, bounded response capture, existing
failure/retry policy, redaction and canonical traffic observability.

Future verification should cover JSON and YAML: equals:null and notEquals:null
as sole operands, explicit-null equality plus every other operator, invalid
null bool/list operands, zero/two operators, duplicate/unknown keys, and typed
serialization/deserialization round trips. Feed admitted predicates into the
actual classifier and retain ForwardAuth's Success/BodyFail distinction.
Existing success tests cover non-null arity and missing-pointer semantics but
not explicit-null presence. Their bodies were read, not executed.

Original current feature/schema and connector documentation require exactly one
operator key and strict serde_json::Value comparisons. Review knowledge and
existing findings were searched for BodyPredicate, RawBodyPredicate, SuccessSpec,
arity and null; no accepted exclusion or duplicate was found. The shared
outbound TLS/Service classifier constraints are unrelated and remain intact.
Pinned local serde_core 1.0.229 OptionVisitor and serde_json 1.0.151
deserialize_option source confirm the null-to-None mechanism; these reads do not
claim a fresh build or runtime reproduction. No implementation, test or build
was run by this review.
