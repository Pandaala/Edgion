---
name: gateway-api-request-transform-validation-and-preparation
description: Preserve CRD-derived RequestRedirect and URLRewrite validation, sibling retention, and immutable fail-closed Gateway preparation.
knowledge_type: regression-guards
source_issue: complete-gateway-api-request-transform-validation-and-preparation-parity
---

# Gateway API request-transform validation and preparation

Source findings: `complete-gateway-api-request-transform-validation-and-preparation-parity`,
`NULL-DEFAULT-PARITY`.

## Invariant

RequestRedirect and URLRewrite constraints have one shared source contract generated
from the vendored Gateway API v1.5 HTTPRoute CRDs. Controller handlers, Admin
validation, EdgionPlugins validation, delegation output, and Gateway defensive
construction must use that shared resource-layer implementation. Do not reproduce
CRD-covered scheme, port, status, path-modifier, pairing, list, or conflict rules
in consumers. Transport-only URI preparation remains an Edgion safety check.
Cross-scope rule/backend conflicts and effective-rule prefix/backend safety are
also centralized there as Edgion context checks; they are not represented as
standalone CRD fragments.

The committed contract contains selected OpenAPI fragments, transform-referencing
CEL expressions, the full list-level schema envelope, defaults, and complete CEL
inventories for each inspected scope. Its generator requires the executable
Standard and Experimental transform contracts to agree. Any scoped CEL change is
visible in the committed artifact, and `--check` must pass after a CRD update. This
is a scoped contract adapter, not a complete Kubernetes admission emulator.
For non-nullable object properties, explicit `null` is pruned before generated
defaults are applied, matching Kubernetes structural-schema defaulting. Typed
HTTPRoute path matches serialize those resolved defaults across conf_sync.

## Publication and runtime behavior

- Authored invalid rules receive Controller invalid-rule status; valid sibling rules
  remain publishable.
- Effective delegated rules are checked after flattening. Authored list cardinality is
  not reapplied to the flattened representation, but prefix transforms require usable
  PathPrefix context.
- Gateway route publication independently drops an invalid rule if malformed state
  bypasses the Controller.
- Request plugins consume `PreparedRequestRedirect` or `PreparedUrlRewrite`; the hot
  path does not parse schema values, status codes, ports, schemes, or replacement paths.
- A missing prefix context or failed URI/header mutation returns a fixed 500 and never
  continues upstream. Errors are static and contain no submitted values; do not add a
  transform-specific tracing path.

## Verification

Run:

```bash
python3 cicd/checks/generate_gateway_api_transform_contract.py --check
python3 cicd/checks/test_generate_gateway_api_transform_contract.py
cargo test -p edgion-resources request_transform --lib
cargo test -p edgion-resources crd_contract --lib
cargo test -p edgion-controller request_transform_validation_preserves_valid_siblings --lib
cargo test -p edgion-controller inherited_prefix_transform_drops_only_child_with_non_prefix_match --lib
cargo test -p edgion-gateway invalid_request_transform_is_dropped_without_hiding_valid_siblings --lib
cargo test -p edgion-gateway gateway_api_filters::request_redirect --lib
cargo test -p edgion-gateway gateway_api_filters::url_rewrite --lib
```

Re-evaluate this guard when the vendored Gateway API version changes, selected
Standard and Experimental fragments diverge, delegation changes its flattened rule
shape, or another resource begins using the generic CRD-contract engine.
