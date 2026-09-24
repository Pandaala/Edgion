# Gateway API validation current state

## Ingress behavior

| Ingress | Structural/schema validation | TypeMeta/version handling | Semantic validation and status |
|---------|------------------------------|---------------------------|--------------------------------|
| Kubernetes | API server applies the installed CRD's OpenAPI schema, pruning/defaulting rules, and CEL | Watchers use statically selected or probed GVKs; preflight checks served availability, not bundle compatibility | Controller handlers validate references and behavior, then patch native status |
| Non-Kubernetes Admin write | Raw source TypeMeta check before typed parse, then source-aware schema validation; HTTPRoute additionally runs the shared generated request-transform contract | Canonical v1 sources and four implemented v1beta1 adapters are inventoried | Schema or transform-contract failure returns 400; accepted resources later enter the normal handler/status path |
| Validation endpoint | Source-aware schema selection and typed validation | Same raw source identity boundary | Returns schema errors plus side-effect-free business validation for EdgionPlugins and HTTPRoute request transforms |
| Filesystem and etcd direct write | Serde parse after raw source identity check; complete local CRD `SchemaValidator` is intentionally not run in watcher paths under the current accepted design | Four implemented v1beta1 adapters convert into canonical models | Parse failures use error sidecars; parsed objects use handler validation/status |
| Gateway conf-sync ingest | No API-server admission; selected HTTPRoute request-transform constraints are rechecked from the embedded generated contract | List and Watch reject noncanonical Gateway API TypeMeta before typed serde; no Gateway-side beta adapter | Invalid transform rules are not published; constructors consume prepared immutable transform state and fail closed if malformed state bypasses the Controller |

The accepted rationale for not running the complete CRD validator in direct
filesystem/etcd watchers is recorded in
`skills/04-review/resources/intentional-design/schema-validator-non-k8s-asymmetry.md`.
Any future change must re-evaluate that decision explicitly rather than treating
it as an unexamined omission.

## SchemaValidator limitations

`edgion-controller/src/conf_mgr/schema_validator.rs` uses a source-aware
registry for Admin/validation paths. The remaining limits are:

- local schema files are pinned to the repository's v1.6.2 bundle profile;
- generic JSON Schema evaluation does not reproduce API-server defaulting or
  pruning;
- uses the standard JSON Schema engine, which does not execute Kubernetes
  `x-kubernetes-validations` CEL rules.

It is therefore an Admin API UX validator, not a general Gateway API version
support registry.

The channel-specific same-GVK distinction is observable within HTTPRoute itself: the v1.6.2
Standard and Experimental bundles both define `HTTPRoute/v1`, but the latter
also admits `useDefaultGateways`, retry, session persistence, and ExternalAuth.
A kind-only or even GVK-only lookup cannot select the intended field contract
unless bundle/channel policy is part of schema registration.

## Existing conversions

- TLSRoute uses v1 only with raw TypeMeta validation, one canonical watcher,
  and the same v1 identity for CRUD and status. The old conversions and
  fallback selection have been removed.
- ReferenceGrant has v1 and v1beta1 Rust types. Kubernetes probing can select a
  v1beta1 watcher whose handler converts into the v1 store representation.
- BackendTLSPolicy was intentionally promoted to v1-only with raw TypeMeta
  guards at filesystem, etcd, Admin, watch, CRUD, and status boundaries.
- Other Gateway API versions served by the bundled CRDs generally rely on API
  server conversion when submitted to Kubernetes and do not have Edgion-owned
  non-Kubernetes adapters.

## RequestRedirect and URLRewrite scoped contract

RequestRedirect and URLRewrite use a generated, committed contract extracted
from the vendored Gateway API v1.6.2 HTTPRoute CRDs. The generator selects their
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

- The offline release matrix runs minimal OpenAPI fixtures for 13 exact stable
  releases in both channels, with per-source N/A rows and a separate
  HTTPRoute field-path presence inventory. It does not prove behavior for
  all same-GVK fields, CEL, or cross-release semantic compatibility.
- The shared Rust fixture test decodes and preparses nine canonical models
  and decodes/converts four wired v1beta1 adapters. It does not run a
  Controller/Gateway end-to-end fixture for each historical release.
- Exact Standard release-pair fixtures cover four changes: HTTPRoute match
  count, newly optional Gateway address value, expanded BackendTLSPolicy
  named CA, and TLSRoute hostname count. Two formerly whole-kind decode
  failures are now per-object unsupported outcomes: missing Gateway address
  value withdraws listeners, and non-System named CA withholds the policy.
  Mixed List and recovery tests cover sibling isolation; this remains narrower
  than full cross-release semantic compatibility.
- `SupportedVersion=Unknown` reflects the lack of installed bundle/channel
  verification. There is no true/false compatibility assessment for a live
  cluster without additional approved discovery or startup locking.
- No executable inventory relates HTTPRoute field stability to Rust type
  coverage, semantic implementation, and `supportedFeatures` declarations.
