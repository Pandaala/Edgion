---
name: fixed-crd-field-coverage
description: Review fixed spec/status property coverage without closing intentionally open CRD bodies.
knowledge_type: regression-guards
---

# Fixed CRD fields have explicit bidirectional coverage

## Conclusion and scope

The resource CRD guard inventories all nine custom CRDs and compares fixed `spec`
and `status` property names in every served version. See the canonical
[schema and transport contract](../../../01-architecture/05-resources/00-schema-transport-contract.md)
for the resource-by-resource scope and deliberately open bodies.

Operator fields in structural objects must be declared, and controller-resolved
fields must remain schema-hidden. Intentionally preserved, undeclared reference
interiors are separate boundaries: an undeclared field there is not evidence of
pruning. Partially declared ACME gatewayRef still checks its name; its five
preserved optional omissions have exact paths and a preserve-unknown gate.

## Rationale and evidence

`crd-fixed-envelope-field-coverage` identified a test gap: the old general walker
compared only four structural specs, while plugin envelopes and status schemas
had narrow pins. `edgion-resources/tests/crd_coverage.rs::fixed_field_scopes`
now pairs generated spec/status schemas with each committed CRD and explicit
opaque boundaries. File-set equality rejects an unregistered custom CRD.

`fixed_projection` follows local reference chains, retains reference siblings,
traverses array items and merges composition branches into a property-name tree.
The existing lockstep diff then reports missing and stale names at the highest
absent ancestor. Scalar leaves become empty property trees, so replacing an
object with a scalar cannot hide its missing fixed children. Invalid references
and reference cycles fail instead of silently reducing coverage.

The expanded guard exposed two structural drifts: ACME privateKeySecretRef and
externalAccountBinding.keySecretRef lacked group/kind, and four HTTP stages plus
Stream plugins retained conditionEnable with no Rust field or runtime consumer.
The CRDs now declare the four existing reference fields and remove the five
obsolete entries. No required/default/type-list or open-config policy was changed.

Open variant config bodies remain opaque, including LinkSys's partly declared
config. HTTP condition references and ACME autoEdgionTls parentRefs items are
explicitly preserved, undeclared reference bodies. Their containing property
must exist; removing preserve-unknown or adding declared children requires the
coverage scope to be re-evaluated. Never infer a blanket skip from a CRD marker.

## Alternatives and limits

This is a static property-name union, not JSON Schema evaluation. It does not
prove type, required-field, default, enum, bound, branch or live Kubernetes
admission parity. Dynamic map keys and map value schemas are excluded. Preserve
the dedicated structural, plugin discriminator, condition and scalar-contract
guards; their contracts are not replaced by this test.

Do not make variant config bodies structural or reintroduce plugin type
whitelists to simplify comparison. Do not exempt actual structural drift just
because the new guard discovers it.

ACME group/kind property declarations do not add support for other credential
resources. The separate [target identity guard](../../acme/regression-guards/secret-reference-target-identity.md)
records runtime rejection of explicit unsupported metadata before Secret lookup.

## Verification and re-evaluation

The initial run passed 27 tests and failed complete parity on the four missing
and five stale fields above, demonstrating the guard against the actual drift.
After the minimal CRD correction, `./cicd/checks/check_crd_coverage.sh` passed
28 field-coverage tests and 8 condition tests. The final combined run of eight
resource targets passed 62 tests: field coverage, conditions, HTTP/Stream
discriminators, status envelopes, open extension types, scalar contracts and
current-status transport. Post-correction workspace all-target check/Clippy,
formatting, documentation, SSA, metrics, Pingora source, Gateway API transform,
unit-test layout and open-extension guards passed. Independent final review of
the applied CRD/doc changes returned LGTM and independently reran the latest
field-coverage and condition test binaries: 28 and 8 passed respectively.
No live Kubernetes admission or runtime target-validation parity is claimed.

In-memory mutation specimens exercise missing/stale top-level and nested fixed
fields, config-body preservation, and missing config roots. Synthetic specimens
exercise composed schemas, nullable arrays, chained refs and scalar replacement.
Re-evaluate on a new CRD/version, changed schema-generator representation,
new fixed fields inside an opaque boundary, or changed preservation semantics.

## Resolution references

- `crd-fixed-envelope-field-coverage` — fixed-field coverage gap and the structural
  drifts it exposed corrected and verified; runtime target validation remains
  covered separately by the target identity guard above.
