---
id: ARCH-061
status: candidate
kind: behavior-defect
priority: P2
partition: PART-204
analyst: /root
---

# Reject multiple custom-field source keys before typed selection

## Current ownership and evidence

`edgion-resources/src/resources/common/outbound_request.rs:350-374` declares
`CustomField { name, source }`, documenting exactly one of `value` or `template`.
The source is an externally tagged `CustomSource` enum flattened into a derived
`Deserialize` struct. The same public field shape serves custom arguments,
headers, cookies and form-body fields. The operator contract explicitly repeats
this exactly-one rule in `skills/02-features/03-resources/18-link-sys.md:360-362`.

The pinned serde 1.0.229 `src/private/de.rs:3228-3245` flattened enum decoder walks
the collected map and returns immediately for the first recognized variant.
`serde_derive-1.0.229/src/de/struct_.rs:327-371` extracts that flattened enum;
it rejects remaining collected fields only when the outer struct explicitly uses
`deny_unknown_fields`, which CustomField does not. Thus a second recognized source
is discarded rather than rejected. The source enum being exhaustive does not
establish input arity.

For a direct JSON decoder, both of these structurally invalid custom fields can
produce a typed field rather than an error:

```json
{"name":"X-Source","value":"literal","template":"${ctx:unused}"}
{"name":"X-Source","template":"${ctx:unused}","value":"literal"}
```

The first selects a literal; the second selects a template when decoder input
order is retained. A Value-based ingress may reorder keys and consistently select
one branch; that does not satisfy the requirement to reject two sources. This is
static evidence from the pinned decoder, not an executed probe or a claim about
ordering at every ingress. A duplicate source key can likewise be discarded at
this direct flattened boundary; earlier generic Value buffering may already
collapse duplicates and is outside this finding's proposed scope.

## Actual call chain and consequence

`OutboundRequestSpec::validation_error`, `visit_template_fields`, and
`collect_secret_ref_names` operate after decoding and inspect only the chosen
typed source. Their exhaustive `CustomSource::Template` matches cannot recover
an ignored branch. `WebhookServiceConfig::get_validation_error` delegates to the
shared request validation, and its Secret readiness collector uses the same
surviving request shape. The managed outbound client readiness gate also consumes
that canonical collector. Gateway `CompiledRequest::from_spec` and
`compile_list_fields` compile only the surviving source, then runtime dispatch
uses literal or rendered-template behavior accordingly.

A configuration accidentally supplying both sources can therefore be admitted
and emit one value even though it violates the declared source contract. An
ignored template is neither validated nor collected for owning Secret binding.
This does not establish an authorization bypass: the surviving source still has
its normal admission and provenance checks. No deployment incident, leaked
credential or measured performance impact is claimed.

LinkSys's CRD preserves opaque provider-config fields, and plugin configuration
interiors are also open. The Resources schema-key detector is advisory and is not
a JSON-Schema oneOf validator. Its ambiguous-union path intentionally unions
recognized keys, while `LinkSysSpec::deserialize` stores findings and still decodes
the typed spec. This valid detector policy cannot enforce recognized-source arity.
Existing source tests cover each single-source form and malformed selected
forms; the inspected common outbound tests do not cover both sources together.

## Bounded improvement and alternative

Give CustomField a local explicit Deserialize implementation that reads the
source keys before constructing CustomSource and requires exactly one recognized
source occurrence. A deserialize-only named wire helper or direct MapAccess
visitor is sufficient; retain the public typed enum, flattened Serialize output
and existing schema shape. Each present source must be a string. Reject present
null, wrong types, both source keys, no source key, and duplicate recognized keys
when visible at this boundary. Do not use ordinary Option<String> decoding in a
way that turns explicit null into absence. Preserve the existing flattened
source decoder's strict JSON/YAML scalar typing: a plain String field or
next_value::<String> is insufficient for direct YAML because serde_yaml
deserialize_string accepts scalar text without checking its resolved type. A
small private present-value decoder using deserialize_any can retain string
inputs while rejecting null, numeric, boolean, sequence and map inputs before
wrapping the result. Preserve the existing decoder-supported string
representations, quoted or explicitly string-tagged YAML values, required-name
decoding and its direct duplicate behavior; do not change the name field's
existing String decoding policy.

Preserve the existing treatment of unrelated unknown fields and the canonical
advisory detector. The improvement needs no generic unknown-key framework,
additional resource clone, request-path validation, wrapper source type or new
Secret mechanism. Rejecting only at OutboundRequestSpec::validation_error is too
late because the second source is already gone. Adding deny_unknown_fields to
all containing types would unnecessarily change unrelated ingestion policy;
explicit local source arity expresses the actual invariant.

The benefit is deterministic rejection of ambiguous operator intent at the
shared configuration boundary. All existing consumers inherit the correction
without new runtime policy. Pre-release rules allow invalid multi-source input
to fail without a compatibility fallback. No valid single-source input needs a
migration.

## Invariants and future verification

Preserve the selected valid literal/template semantics, header framing guards,
form encoding, existing rendering policies, trusted-variable admission, canonical
SecretRef lexical parsing, owning resource/ReferenceGrant authority, Secret
redaction and readiness-before-cache-reuse. Keep main responses streaming and
avoid any new storage, observability or request-path allocation.

Future implementation checks should exercise both source orders for actual
CustomField and its arguments/header/cookie/form placements; JSON/YAML direct
inputs and Value-based containing-resource inputs; no-source, explicit-null,
wrong-type and duplicate-visible-key cases (including YAML numeric, boolean
and null scalars versus their quoted string equivalents); valid literal and template round
trips; unrelated unknown-field advisory behavior; and unchanged canonical owning
Secret collection for a valid template. A distinguishing malformed ignored
branch should fail during decoding rather than disappear before validation.
Place tests in the existing production-crate unit-test tree. Existing tests were
read only and are not fresh execution of a fix.

## Original knowledge, deduplication and independent review

Read original root/graded rules, Resources schema/transport contract, request
runtime and owning-template review decisions, credential unknown-field tradeoff,
and serde-ignored/schema-walk authority. The latter prohibits replacing the
canonical detector with serde_ignored; this recommendation does not do so.
ARCH-057 concerns explicit JSON null in success predicates, whereas this cause is
first-variant selection by a flattened CustomSource decoder. The owners, invalid
inputs and corrections are separate. ARCH-051's retired WebhookAuth surface and
ARCH-050's repeated cold plugin preparation do not enforce custom-field arity.

Baseline primary hash:
`dc617145699d02dd4db00a465e4cac7e4afbe39f4d89bbd6ba846037e05651be`.
The source-complete PART-204 report records supporting scopes and hashes.
Independent verification used original rules, current sources and pinned decoder
implementations; no runtime reproduction is claimed. No code, tests, existing
documentation or Git state was modified.
