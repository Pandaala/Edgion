---
name: header-modifier-validation
description: Review HTTP and gRPC header modifier admission, construction failure, and update recovery.
knowledge_type: regression-guards
---

# Header modifiers validate before runtime publication

## Conclusion and scope

The shared `resources/header_modifier.rs` validates `HTTPHeaderFilter` and
`GRPCHeaderFilter` at configuration time. Their `set` and `add` fields hold
configured name/value operations; `remove` holds names to delete.
Names use the canonical Gateway API token predicate and 256-byte bound.
Values have 1–4096 Unicode scalar values and must parse as HTTP header values.
Each action list contains at most 16 entries. Cross-action equivalent names are
rejected. Within one list, first-equivalent execution remains defensive behavior,
but every configured entry, including an ignored duplicate, is validated.
This first-equivalent execution statement describes the ordinary native plugin
plan. AI backend dispatch still applies valid schema entries directly, including
same-list duplicates; that existing parity gap remains in the preparation task.

Native HTTP/gRPC filter lists require matching modifier type/config fields and
at most one modifier of each direction per list. Rule and backend lists are
separate scopes. This check does not generalize validation of unrelated filter
payloads or impose a new total filter-list limit on delegated ExtensionRefs.

## Rationale and evidence

Authority: the vendored Gateway API v1.5 standard schema and
[upstream HTTP types](https://github.com/kubernetes-sigs/gateway-api/blob/v1.5.0/apis/v1/httproute_types.go).
The experimental printable-ASCII pattern is not part of the standard value
profile. The task explicitly preserves first-equivalent entries despite the
broader header-filter prose about duplicate actions; this is a defensive
non-native-admission behavior, not a relaxation of Kubernetes admission.

Previously `EdgionPlugin::validation_error` returned no error for either
modifier, native builders silently skipped missing payloads, and constructors
only deduplicated strings. Invalid retained values failed during request
execution while the filter continued.

- Shared plugin validation now calls the same `validate()` as Gateway modifier
  constructors. Disabled EdgionPlugins entries remain inert.
- Controller HTTP/gRPC `find_invalid_rule_*` calls `header_modifier_error()`
  for rule and backend lists. Parent/child delegation uses those same checks.
  Valid sibling rules survive. Authored header errors reject attachment when no
  effective rule survives. Existing all-empty gRPC child-delegation status
  semantics are unchanged; the empty effective table still publishes no rule.
- Gateway native rule/backend builders validate independently, record static
  invalid-config names, and seal the request chain using `RejectInvalidConfig`.
  The gRPC constructors now seal as well. EdgionPlugins reuse the existing
  non-retainable invalid-update behavior; policy Block/Bypass ordering is unchanged.
- AI dispatch applies backend headers directly, so it checks the compiled
  backend runtime's persistent `rejects_invalid_config()` outcome before any
  header mutation. `has_invalid_plugins()` is a pre-seal construction query:
  sealing consumes its error list, so it cannot guard direct request consumers.

All configuration errors omit configured names and values. Runtime rejection
uses the existing generic 500 and canonical plugin/access-log diagnostics.
No tracing, metric, response buffering, or new failure-policy knob is added.

## Alternatives and limits

Only adding Controller status validation would leave Gateway construction
unguarded. Only checking values when writing headers would still allow partial
execution. Both are insufficient.

Runtime string preparation is deliberately unchanged in this fix. The user
selected the validation-only subset; the recorded follow-up identifier is
`prepare-gateway-api-header-modifier-runtime.md`. That task is absent from the
current tree; its absence does not establish completion of the deferred work.
Request framing and response-field restrictions remain in their separate tasks;
this correction does not establish a local restricted-header list.

The existing [multi-value append rule](../../http1/regression-guards/append-request-header-multi-value.md)
still applies: append preserves all existing same-name field lines. The prior
[ExternalAuth sealing correction](gw-plugin-runtime-01-httproute-external-auth-invalid-plugins-never-sealed.md)
is relevant construction precedent, not evidence that HeaderModifier was already safe.

## Verification and re-evaluation

- Shared resource tests: `cargo test --offline -p edgion-resources --lib header_modifier`,
  6 passed in this run.
- Controller: `cargo test --offline -p edgion-controller --lib header_modifier`,
  9 passed. The freshly built test binary also passed the complete HTTP handler
  (40), gRPC handler (20), and delegation (71) groups.
- Gateway: `cargo test --offline -p edgion-gateway --lib header_modifier`,
  19 passed, including AI direct-consumer rejection and request/response
  valid-to-invalid-to-valid publication. The freshly built test binary also
  passed runtime (47), plugin store (37), and AI dispatch (23) groups.
- The initial Gateway run exposed an invalid test/guard assumption that
  `has_invalid_plugins()` survived sealing. The persistent sealed outcome
  corrected that assumption; the counts above are the successful rerun.
- `cargo fmt --all`, `cargo fmt --all -- --check`,
  `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` passed. Clippy still emits existing
  repository warnings; its JSON diagnostics had no primary warning/error span
  overlapping this change's added/modified lines.
- Agent-document, SSA force, metrics inventory, Pingora source-policy, Gateway
  no-tracing, and `git diff --check` guards passed. Existing integration files
  were unchanged; no full-workspace test or live integration run is claimed.
- Independent read-only code and knowledge review returned LGTM.

Re-evaluate when a new configuration source, native filter, delegation merge,
constructor, AI backend consumer, or update policy bypasses these boundaries.
Tests cover syntax/bounds, HTTP/gRPC parity, pairing, invalid siblings,
delegation, static errors, execution ordering, and publication/recovery.

## Resolution references

- `complete-gateway-api-header-modifier-validation-and-preparation-parity`:
  validation-only correction verified on 2026-09-11 after user scope approval;
  typed preparation and the documented AI duplicate-execution parity gap remain
  deferred under the recorded follow-up identifier, not fixed or accepted as unnecessary.
