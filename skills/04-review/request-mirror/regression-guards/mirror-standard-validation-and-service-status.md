---
name: mirror-standard-validation-and-service-status
description: Use when reviewing RequestMirror standard sampling fields, Service identity and port validation, missing-Service status, or Service-triggered requeue for HTTPRoute and EdgionPlugins.
knowledge_type: regression-guards
finding_id: complete-request-mirror-standard-validation-and-service-status
---

# RequestMirror standard validation and Service dependency lifecycle

## Conclusion and scope

`HTTPRequestMirrorFilter::validation_error` is the canonical validation for
standard RequestMirror fields outside Kubernetes admission. Both the
EdgionPlugins Controller/Gateway validation path and HTTPRoute rule validation
reuse it. Neither `fraction` nor `percent` means 100% mirroring; the two fields
are mutually exclusive. Fraction denominator is at least 1, numerator is in
`0..=denominator`, and percent is in `0..=100`. A core Service reference has a
non-empty name and a required port in `1..=65535`.

Invalid HTTPRoute mirrors drop their owning rule through the existing
invalid-rule/`PartiallyInvalid` behavior. Invalid EdgionPlugins mirrors set
`Accepted=False/Invalid` and use the same diagnostic text in Controller and
Gateway compilation. A valid reference to an absent Service sets
`ResolvedRefs=False/BackendNotFound` for both publishers. Existence is checked
before reporting a stored cross-namespace denial: `RefNotPermitted` applies to
an existing target, while an absent target remains `BackendNotFound`.

## Rationale and evidence

The authoritative field and referent requirements are in the vendored Gateway
API v1.5 HTTPRoute CRD. Kubernetes schema and CEL validation do not cover file,
etcd, Admin API, or the preserved EdgionPlugins config object, so validation in
the shared resource type is load-bearing.

- `edgion-resources/src/resources/http_route.rs` owns the standard field shape,
  validation, and rule/backend-filter traversal.
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/request_mirror.rs`
  composes standard validation with the existing tuning limits.
- Controller `handlers/http_route.rs` feeds the shared rule result into the
  existing invalid-rule status path. Delegation uses that same rule validation
  before publishing resolved rules.
- Gateway `routes/http/conf_handler_impl.rs` independently refuses an invalid
  effective rule, including controller-resolved delegation input.
- Controller `handlers/request_mirror.rs` checks Service existence through the
  canonical processor registry and emits the existing `BackendNotFound` error.

Mirror Service dependencies use the existing `service_ref_mgr` and serial
workqueues. Their source key is role-qualified, while forwarding backends retain
the canonical `namespace/name` key. Service and EndpointSlice events merge the
two roles and deduplicate consumers. Backend policy ancestor discovery continues
to read only forwarding keys because RequestMirror does not support
BackendTLSPolicy; a mirror target must not become a false policy ancestor.
Apply reconciliation clears all prior keys before registering the current
generation, and delete clears the resource's keys. Service requeue runs from
`after_publish` and `after_unpublish`, so dependent status evaluation observes
the newly published Service or its confirmed absence rather than the preceding
cache generation. HTTPRoute mirror registration uses the same namespace-normalized
owner identity as forwarding registration and deletion; file input with an
omitted namespace therefore cannot leave a `request-mirror:` edge under a raw
namespace-less owner.

## Alternatives and limits

Relying on Kubernetes admission leaves other configuration centers inconsistent.
Clamping invalid sampling at request time can silently turn a typo into no
mirroring or full mirroring. Adding mirror targets under the forwarding Service
key would requeue correctly but would incorrectly expose them to backend policy
ancestor resolution. A second dependency manager is unnecessary and would
duplicate the existing cross-resource mechanism.

This correction does not add mirror BackendTLSPolicy support, change endpoint
selection, modify request-body handling, or make mirror failures affect the
primary request. Unsupported non-Service target kinds fail the shared standard
validator: an HTTPRoute drops the owning rule through `PartiallyInvalid`, while
an EdgionPlugins resource reports `Accepted=False/Invalid`.

## Verification and re-evaluation

Targeted resource tests cover standard sampling, filter/config pairing, core
Service identity, name, and port. Controller tests cover rule/backend-filter
rejection, missing-Service error mapping, both publisher registrations,
replacement/delete cleanup, role-key isolation, and post-publish/unpublish
Service visibility. The HTTPRoute lifecycle matrix also covers omitted-namespace
file input and asserts both dependency directions after replacement and delete.
Gateway tests cover Controller parity and the
effective/resolved-rule publication backstop.

Fresh closure verification passed `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets`,
the agent-doc, SSA-force, metrics-inventory, and Pingora-source guards, and
`./cicd/checks/check_crd_coverage.sh` (16 CRD coverage plus 8 plugin-condition
coverage tests). Clippy emitted one pre-existing `redundant_async_block` warning
in the frozen `edgion-tests` integration tree. The independent read-only review
returned `LGTM` after checking validation, status priority, dependency cleanup,
post-publication requeue ordering, policy-role isolation, workqueue discipline,
and knowledge placement.

Re-evaluate if RequestMirror gains a non-Service target, BackendTLSPolicy support,
a different route rule-admission model, or a replacement for `service_ref_mgr`.

## Resolution references

- `complete-request-mirror-standard-validation-and-service-status` — confirmed
  correctness defect; fixed across shared validation, both publishers, status,
  and Service dependency lifecycle.
