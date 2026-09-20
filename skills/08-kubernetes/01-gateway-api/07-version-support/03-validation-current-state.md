# Gateway API validation current state

## Ingress behavior

| Ingress | Structural/schema validation | TypeMeta/version handling | Semantic validation and status |
|---------|------------------------------|---------------------------|--------------------------------|
| Kubernetes | API server applies the installed CRD's OpenAPI schema, pruning/defaulting rules, and CEL | Watchers use statically selected or probed GVKs; preflight checks served availability, not bundle compatibility | Controller handlers validate references and behavior, then patch native status |
| Non-Kubernetes Admin write | Typed parse, serialize back to JSON, then `SchemaValidator`; HTTPRoute additionally runs the shared generated request-transform contract | BackendTLSPolicy has an explicit raw v1 guard; most kinds do not | Schema or transform-contract failure returns 400; accepted resources later enter the normal handler/status path |
| Validation endpoint | Same typed-parse-before-schema sequence; always invokes the local schema validator | Same raw-version limitations | Returns schema errors plus side-effect-free business validation for EdgionPlugins and HTTPRoute request transforms |
| Filesystem and etcd direct write | Serde parse only; local CRD `SchemaValidator` is intentionally not run in watcher paths under the current accepted design | BackendTLSPolicy is guarded; TLSRoute may convert v1alpha3; most kinds discard submitted TypeMeta | Parse failures use error sidecars; parsed objects use handler validation/status |
| Gateway conf-sync ingest | No API-server admission; selected HTTPRoute request-transform constraints are rechecked from the embedded generated contract | Receives Controller-produced canonical serialized types | Invalid transform rules are not published; constructors consume prepared immutable transform state and fail closed if malformed state bypasses the Controller |

The accepted rationale for not running the complete CRD validator in direct
filesystem/etcd watchers is recorded in
`skills/04-review/resources/intentional-design/schema-validator-non-k8s-asymmetry.md`.
Any future change must re-evaluate that decision explicitly rather than treating
it as an unexamined omission.

## SchemaValidator limitations

`edgion-controller/src/conf_mgr/schema_validator.rs` currently:

- keys validators only by `ResourceKind`;
- selects only `spec.versions[0]` from each CRD;
- scans both standard and experimental bundles, allowing duplicate kinds to
  overwrite one another according to filesystem enumeration order;
- validates the typed re-serialized object rather than the original document;
- skips validation when no schema was registered;
- uses the standard JSON Schema engine, which does not execute Kubernetes
  `x-kubernetes-validations` CEL rules.

It is therefore an Admin API UX validator, not a general Gateway API version
support registry.

The duplicate-GVK problem is observable within HTTPRoute itself: the v1.5.0
Standard and Experimental bundles both define `HTTPRoute/v1`, but the latter
also admits `useDefaultGateways`, retry, session persistence, and ExternalAuth.
A kind-only or even GVK-only lookup cannot select the intended field contract
unless bundle/channel policy is part of schema registration.

## Existing conversions

- TLSRoute has v1 and v1alpha3 Rust resource types, bidirectional conversions,
  version probing, separate compatibility handler wiring, dynamic CRUD
  selection, and filesystem v1alpha3-to-v1 normalization.
- ReferenceGrant has v1 and v1beta1 Rust types. Kubernetes probing can select a
  v1beta1 watcher whose handler converts into the v1 store representation.
- BackendTLSPolicy was intentionally promoted to v1-only with raw TypeMeta
  guards at filesystem, etcd, Admin, watch, CRUD, and status boundaries.
- Other Gateway API versions served by the bundled CRDs generally rely on API
  server conversion when submitted to Kubernetes and do not have Edgion-owned
  non-Kubernetes adapters.

## RequestRedirect and URLRewrite scoped contract

RequestRedirect and URLRewrite use a generated, committed contract extracted
from the vendored Gateway API v1.5 HTTPRoute CRDs. The generator selects their
OpenAPI fragments, transform-referencing filter/list/rule CEL expressions, complete
list-level schema, and defaults. It records the complete scoped CEL inventories,
requires the executable Standard and Experimental contracts to be identical, and
supports `--check` drift detection.

The shared resource layer compiles that contract once, applies it to authored
rule/backend filter lists and EdgionPlugins configs, and prepares typed immutable
Gateway state. Effective delegated rules receive a separate safety pass because
flattening can legitimately change authored list cardinality while still needing
valid prefix-match context. Filesystem and etcd still report failures through
the normal handler/status lifecycle; they do not gain a pretend admission gate.

This is deliberately a selected semantic contract, not a claim that Edgion
faithfully emulates every Kubernetes admission behavior. Typed deserialization
still precedes this check outside Kubernetes, and the generic local
`SchemaValidator` still does not execute arbitrary CRD CEL. Extend the generator's
scoped extraction when another resource or field family needs the same model;
do not copy CRD constraints into hand-written validators.

## Missing verification

- No guard proves that the declared source-version matrix matches typed
  resources, watcher/CRUD/status registrations, and converters.
- No cross-bundle tests establish how the canonical Rust model handles older or
  newer standard HTTPRoute v1 schemas.
- No generic source-GVK negative matrix covers missing, malformed, or
  unsupported TypeMeta across Admin, filesystem, and etcd.
- No test makes `SupportedVersion` false for an incompatible installed bundle.
- No deterministic duplicate-GVK policy exists for standard versus
  experimental schema files.
- No executable inventory relates HTTPRoute field stability to Rust type
  coverage, semantic implementation, and `supportedFeatures` declarations.
