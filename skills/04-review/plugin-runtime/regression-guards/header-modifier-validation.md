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
AI backend dispatch borrows the same prepared plan as the ordinary native HTTP
backend plugin. It also retains only the first equivalent operation within each
list; deduplication applies to configured operations, never incoming field lines.
The plan stores each retained `set`/`add` value as a prevalidated `HeaderValue`.
Its backing bytes are promoted to shared storage during publication, so HTTP,
gRPC, and AI request execution clone the typed value without allocating or
copying its payload.

Native HTTP/gRPC filter lists require matching modifier type/config fields and
at most one modifier of each direction per list. Rule and backend lists are
separate scopes. This check does not generalize validation of unrelated filter
payloads or impose a new total filter-list limit on delegated ExtensionRefs.

## Rationale and evidence

Authority: the vendored Gateway API v1.5 standard schema and
[upstream HTTP types](https://github.com/kubernetes-sigs/gateway-api/blob/v1.5.0/apis/v1/httproute_types.go).
The experimental printable-ASCII pattern is not part of the standard value
profile. The schema's header-name description requires first-equivalent
execution. Its set/add list-map keys use the original `name`, and remove uses
original string set entries: differently cased names can pass native admission.
Exact duplicate map/set keys are rejected there, but can arrive through other
configuration sources. Every entry must still pass shared validation, and
cross-action equivalent names remain invalid.

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
- The native HTTP backend builder retains an `Arc` to the request modifier
  already installed in its request chain. AI borrows that modifier's immutable
  `HeaderModifierPlan` instead of executing `schema.filters` action lists.
  Each replacement runtime owns its own plan; captured older generations remain
  unchanged. No per-request preparation or second duplicate-selection rule is added.
- Header modifier construction converts retained values to `HeaderValue` and
  performs the first `Bytes` clone while publishing the immutable plan. That
  first clone promotes owned bytes to shared storage; later request-path clones
  increment the shared reference without allocating or copying the value bytes.
  The four HTTP/gRPC request/response consumers and AI backend dispatch use the
  existing typed-value `PluginSession` methods.

All configuration errors omit configured names and values. Runtime rejection
uses the existing generic 500 and canonical plugin/access-log diagnostics.
No tracing, metric, response buffering, or new failure-policy knob is added.

## Alternatives and limits

Only adding Controller status validation would leave Gateway construction
unguarded. Only checking values when writing headers would still allow partial
execution. Both are insufficient.

Header values are prepared once, but configured header names intentionally remain
strings. Preserving their authored spelling matters on HTTP/1, while Pingora's
case-preserving insertion API converts even typed names back through its internal
name parser. Changing that boundary would require a wider `PluginSession` and
Pingora-fork API change for smaller gains. This correction therefore claims
zero allocation and zero payload copy only for cloning each prepared value, not
for the complete header-map insertion path.
Request framing and response-field restrictions remain in their separate tasks;
this correction does not establish a local restricted-header list.

The existing [multi-value append rule](../../http1/regression-guards/append-request-header-multi-value.md)
still applies: append preserves all existing same-name field lines. The prior
[ExternalAuth sealing correction](gw-plugin-runtime-01-httproute-external-auth-invalid-plugins-never-sealed.md)
is relevant construction precedent, not evidence that HeaderModifier was already safe.

## Verification and re-evaluation

### Historical validation-only verification (2026-09-11)

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

### Scoped duplicate-execution verification (2026-09-20)

- `cargo test --offline -p edgion-gateway --lib header_modifier`: 21 passed.
  The same freshly built test binary passed all 25 `ai::dispatch::tests` tests.
  New assertions compare ordinary backend execution with AI for case-equivalent
  and exact duplicate operations, existing multi-value fields, rejected invalid
  duplicate values, recovery, and captured older generations.
- The newly built `test_client -g --dynamic-scenario AiBackendWeighted` passed
  against the shared local Controller/Gateway/test-server topology. Both providers
  observed initial, updated, restored, and removed modifier generations, preserving
  existing add values and applying only the first equivalent configured operation.
  The topology was stopped afterward. No Kubernetes or full integration run is claimed.
- Formatting, workspace all-target check and Clippy passed. Clippy reported only
  existing `redundant_async_block` and `err_expect` warnings in unchanged test code.
  SSA-force, metrics inventory, Pingora source-policy, Gateway no-tracing, and
  `git diff --check` guards passed. Independent read-only code and knowledge review
  returned LGTM after correcting the native-admission scope description above.
- Global agent-document validation remains blocked by an unrelated existing link
  in `listeners/intentional-design/linux-host-tuning-baseline.md` to the absent
  `tasks/todo/os-packaging-and-system-service-adapters.md`. That page was not changed
  by this correction. At that point the original preparation task remained open
  for typed-runtime work; this scoped fix did not claim full task closure.

### Typed-value preparation verification (2026-09-20)

- `cargo test --offline -p edgion-gateway --lib header_modifier`: 22 passed.
  Coverage includes HTTP/gRPC request and response execution, AI backend parity,
  validation and publication recovery, operation ordering, first-equivalent
  selection, and an allocation-counter assertion proving that a prepared
  `HeaderValue` clone performs zero allocations after publication-time promotion.
- All 25 `ai::dispatch::tests` and all 52 `gateway_api_filters` tests passed.
  Independent read-only code and knowledge review returned LGTM.
- Formatting, workspace all-target check, Clippy, agent-doc, SSA-force,
  metrics-inventory, and Pingora-source guards passed. The required Gateway API
  transform-contract command could not run because its script is absent from the
  repository and Git history, so the source task remains open. No full-workspace
  test or new live integration run is claimed.

Re-evaluate when a new configuration source, native filter, delegation merge,
constructor, AI backend consumer, or update policy bypasses these boundaries.
Tests cover syntax/bounds, HTTP/gRPC parity, pairing, invalid siblings,
delegation, static errors, execution ordering, and publication/recovery.

## Resolution references

- `complete-gateway-api-header-modifier-validation-and-preparation-parity`:
  validation-only correction verified on 2026-09-11 after user scope approval.
- `prepare-gateway-api-header-modifier-runtime`: AI first-equivalent execution
  was corrected first. The follow-up then moved retained values into the shared
  runtime plan as prevalidated `HeaderValue`s. Typed name preparation was rejected
  as a wider Pingora-boundary change that would not eliminate its internal parse;
  original name spelling and existing transport semantics remain unchanged.
