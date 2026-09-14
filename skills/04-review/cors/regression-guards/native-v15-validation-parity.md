---
name: native-v15-validation-parity
description: Use when reviewing native Gateway API HTTPCORSFilter validation, defaults, Controller status, or Gateway construction.
knowledge_type: regression-guards
---

# Native Gateway API CORS Validation Matches v1.5 on Every Ingest Path

## Conclusion and scope

`edgion-resources::resources::HTTPCORSFilter::validate` is the canonical semantic
validator for native, rule-scoped Gateway API CORS. It enforces the Gateway API
v1.5 origin, method, header, list, wildcard, and `maxAge` contract for every
configuration backend. Omitted `maxAge` deserializes to `5`; an explicit zero is
invalid. The existing Edgion restriction that credentialed native CORS must use
explicit `exposeHeaders` names remains part of the same validation result.

The Controller uses the result to exclude only the invalid rule and to publish an
operator-facing status reason. `GapiCorsFilter::try_new` calls the same validator as
a defensive Gateway boundary. This rule does not apply to the separate
`EdgionPlugins` CORS schema or validator.

## Rationale and evidence

The canonical authority is the vendored Gateway API v1.5.0 HTTPRoute schema in
`config/crd/gateway-api/gateway-api-standard-v1.5.0.yaml` and the corresponding
upstream `HTTPCORSFilter` and `CORSOrigin` definitions. Kubernetes admission applies
that schema, but file-system and other non-Kubernetes ingestion do not. Runtime
semantic validation is therefore required to keep their accepted configuration set
aligned and to validate documented port bounds that are not fully expressed by the
origin regex alone.

The value flow is:

1. Serde constructs `HTTPCORSFilter`, applying the v1.5 `maxAge: 5` default.
2. Controller `invalid_cors_reason` calls `HTTPCORSFilter::validate` and carries its
   field-specific errors into rule status.
3. For ordinary routes, the Controller transports the complete resource plus
   `invalid_rule_indices`; delegated routes transport already-filtered resolved rules.
4. Gateway publication independently rejects invalid effective rules, including when
   Controller invalid indices are absent or do not apply to resolved rules.
5. Gateway `GapiCorsFilter::try_new` invokes the same validator before retaining the
   immutable runtime filter.
6. A successful preflight emits the validated positive `maxAge`, including the
   default value `5`.

Before the correction tracked by
`complete-native-gateway-api-cors-validation-parity.md`, the shared validator checked
only partial origin syntax and credentialed exposure. Non-Kubernetes input could
therefore publish invalid lists, methods, header names, ports, and `maxAge: 0`, while
an omitted `maxAge` silently became zero and suppressed the response header.

## Alternatives and limits

A watcher-local OpenAPI engine is not used: it would duplicate the contract and would
not protect other non-Kubernetes ingestion or the Gateway boundary. The native and
EdgionPlugins CORS admission contracts remain separate even where they reuse header
or port primitives.

Validation uses exact string set semantics, matching Kubernetes list-set admission.
Header names remain semantically case-insensitive at HTTP runtime, but differently
cased configured strings are not collapsed by this validator because Kubernetes list
sets do not perform case-folding. Origin host syntax follows the vendored v1.5 pattern;
port values additionally follow the documented `1..=65535` range.

## Verification and re-evaluation

Targeted unit tests executed during implementation cover omitted and explicit
`maxAge`, valid and invalid origin length/scheme/host/port boundaries, list limits
and duplicates, wildcard exclusivity, allowed methods, header-name grammar,
Controller diagnostic and delegation propagation, Gateway publication and
constructor rejection, and emitted default max age.

Closure verification on 2026-09-14 passed `cargo fmt --all`,
`cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets`,
the agent-documentation validator, SSA force guard, metrics inventory guard, and
Pingora source-policy guard. Clippy completed successfully with one unrelated
pre-existing `redundant_async_block` warning in the integration-test harness. The
targeted resource, Controller, delegation, Gateway CORS, and Gateway publication
tests also passed. The full `cargo test --all` suite was not run for this task.

Re-evaluate when the vendored Gateway API version changes its CORS types or validation,
when a new HTTPRoute ingestion path bypasses `HTTPCORSFilter::validate`, or when the
native credentialed exposure policy changes.

## Resolution references

- `complete-native-gateway-api-cors-validation-parity.md` — fixed in the current task.
- `native-credentialed-exposure-policy.md` — retained Edgion credentialed exposure restriction.
- `native-cors-rule-scope.md` — retained native CORS rule ownership boundary.
