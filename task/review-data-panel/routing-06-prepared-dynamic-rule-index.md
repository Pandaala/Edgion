# Prepared DynamicInternalUpstream rule lookup

Status: deferred optimization decision for the user. Baseline: `181448c8a`.
No runtime change or measured performance improvement.

## Evidence

DynamicInternalUpstream::run_request extracts a routing key, then calls
DynamicInternalUpstreamConfig::resolve_target. In
`edgion-resources/src/resources/edgion_plugins/plugin_configs/dynamic_internal_upstream.rs`,
rules mode scans the Vec in order for exact string equality and returns borrowed
target name/namespace fields from the first matching rule. Duplicate keys are
accepted by check_fields and retain first-wins behavior. Misses scan all rules.

None selects direct mode and returns the input key. Some(empty) is rejected by
check_fields; resolve_target itself defensively returns None for it. The field
comment describing omitted/empty rules as direct mode is stale relative to those
implementations. It is not permission to make an empty configured list valid.

The Gateway constructor clones configuration and calls validate for preparation;
validate returns early on invalid fields and is not a fallible rejecting
constructor. Preserve existing shared validation and runtime compilation gates
without inferring stronger guarantees from that call. Request-side regex capture,
route validation and owned jump publication have separate responsibilities.

## Decision

Keep the linear scan, prepare a Gateway-owned exact-key index at construction,
or evaluate an explicitly measured small-list threshold. None is selected. An
index can avoid later-hit/miss comparisons but adds hashing, publication work and
storage; a one-rule list can favor the current scan. A key-to-index map may copy
keys while retaining the original Vec; moving targets into a runtime map changes
ownership and construction. Decide from measured whole-path costs, not asymptotic
lookup complexity alone. Do not assume a borrowed-key self-reference is sound.

This differs from [routing-01](routing-01-dynamic-internal-input-ownership.md)'s
input lifetime and [dye-02](dye-02-lazy-outcome-value-formatting.md)'s success text.
It changes neither target authorization nor the route's canonical backend matcher.

## Acceptance for a selected implementation

- Preserve exact case-sensitive equality and first-wins duplicate ordering,
  including duplicate keys with different targets/namespaces. No duplicate
  rejection, last-wins insertion, sorting-based precedence or normalization.
- Preserve None/direct mode, invalid empty lists, empty request/capture behavior,
  missing/no-match/onInvalid policy distinctions and validation error precedence.
  Preparation must not silently make an invalid configuration usable or bypass
  canonical regex/entry validation. Add no unrequested rule-count limit.
- Keep no-route/no-backend-ref pass-through, compiled target validation,
  ReferenceGrant denial, jump-lock ordering, success-only dye and existing errors.
  Index membership proves a configured mapping, never permission to use a backend.
- Derived indexes belong to immutable plugin generations. Update every applicable
  constructor and preserve retained old requests during reload. Keep config/schema
  and conf_sync representation unchanged; no serialized derived map, global cache
  or first-request lazy compilation. Freeze an exact implementation allowlist.
- If hashing request-derived keys, preserve appropriate hash-flood protection;
  do not substitute a fast non-resistant hasher without independent review.
  Measure direct mode, one/few/many rules, first/middle/last hits, misses and early/
  late duplicate keys, with/without regex. Report construction cost, added retained
  memory, copied keys/targets and lookup CPU separately from jump/dye allocation.
- Differentially test old/new selected fields and errors, including denied and
  locked targets and hot reload. Keep the shared canonical matcher and recorder;
  introduce no plugin-local authorization, tracing or observability mechanism.
  Require independent IMPL plan/final review and repository checks.

Fresh baseline verification passed ten Gateway DynamicInternalUpstream and six
Resources configuration tests (16 total). This is not index verification or a
benchmark. Original contracts: [named-jump denial](../../skills/04-review/routing/regression-guards/denied-named-jump-fails-closed.md),
[backendRef scope](../../skills/04-review/routing/regression-guards/backendref-upstream-jump-plugin-scope.md)
and [dye producer parity](../../skills/04-review/plugin-runtime/regression-guards/dye-stage-producer-parity.md).
