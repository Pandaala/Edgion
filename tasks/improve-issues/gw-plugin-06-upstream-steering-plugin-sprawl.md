# gw-plugin-06: Five upstream-steering plugins duplicate one skeleton; "Canary" does no canary (weighted) release

- **Category**: over-design
- **Severity**: P3
- **Scope**: large-direction
- **Location**: `edgion-gateway/src/plugins/http/canary/plugin.rs`, `dynamic_internal_upstream/plugin.rs`, `dynamic_external_upstream/plugin.rs`, `direct_endpoint/plugin.rs`, `region_route/plugin.rs`

## Summary
Five plugins each steer the request to a different upstream through
`session.set_upstream_jump(...)`. Four of them re-implement the same skeleton: extract a key
through KeyGet, optionally run a regex capture, look it up in a table, run
`on_missing`/`on_no_match`/`on_invalid` × `Fallback`/`Reject`, call `validate_backend_by_name`,
set the jump, and record the dye outcome. Despite its name, `Canary` is a Selector-driven
whole-route profile switch (blue/green). It has no percentage, header, or cookie split. Real
canary releases rely on Gateway API backendRef weights or `DynamicInternalUpstream`. Operators have
to learn five schemas for one concept ("pick which backend serves this request"). Only the
first-setter-wins `SetJumpOutcome::Rejected` rule arbitrates between them, and it silently ignores
the later plugin.

## Current behavior (evidence)
- Identical extraction and fallback blocks appear in `dynamic_internal_upstream/plugin.rs:29-125`,
  `dynamic_external_upstream/plugin.rs:28-115`, and `direct_endpoint/plugin.rs:35-50,111-205`, with
  near-identical `ErrResponse` bodies and `record_outcome` reasons.
- `Canary::run_request` (`canary/plugin.rs:117-178`) and `DynamicInternalUpstream`
  (`:115-175`) share the `route_unit` / `backend_refs` / `validate_backend_by_name` /
  `InternalJumpPreset` tail verbatim.
- Every plugin treats `SetJumpOutcome::Rejected` as `GoodNext` with only a `jump locked, rejected`
  log, so two steering plugins on one route silently lose to whichever ran first.
- `Canary` has no weight field: the profile is chosen by `active_profile` or the Selector only
  (`canary/plugin.rs:69-108`).

## Industry comparison
- **APISIX `traffic-split`**: one plugin with ordered `match` rules (vars and expressions) and
  `weighted_upstreams`. It covers header routing, blue/green, and weighted canary.
- **Envoy / Gateway API**: header or cookie matches plus `weightedClusters` / backendRef weights.
  Edgion already implements those natively.
- **Kong `canary`** (Enterprise) combines percentage, allow-list, and time-based progression in
  one plugin.

## Impact
Five schemas, five docs, five test matrices, and five sets of review guards for one capability.
Bug fixes to the shared skeleton (regex extraction, fallback semantics, dye outcomes) must be
repeated. The "Canary" name misleads users looking for weighted rollout. Conflicting steering
plugins fail silently.

## Suggested direction
- Extract one shared steering core: key resolution, regex capture, and the fallback policy enum
  with its responses and dye outcomes. Each plugin then only supplies its target resolver
  (backendRef by name, external domain, endpoint IP, region table, Selector profile).
- Rename `Canary` to what it is (`ProfileSwitch` / `BlueGreen`), or give it optional weights so
  the name is honest. Pre-release status allows the rename.
- Make jump conflicts visible: a `SetJumpOutcome::Rejected` from a *configured* steering plugin
  should produce a diagnostic (`EdgionErrorCode`). Better, have Controller validation reject two
  steering plugins in the same effective chain unless explicitly ordered.

## Notes
- `skills/04-review/routing/regression-guards/backendref-upstream-jump-plugin-scope.md` covers jump
  scope, not plugin duplication. No intentional-design page justifies five separate skeletons.
- This is a direction item. No behavior is incorrect today beyond the silent conflict and the
  naming.

## Verification (independent)

- **Verdict**: CONFIRMED (severity P3 unchanged; direction item)
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/canary.rs:23-49`: `CanaryConfig` has `active_profile`, `profiles`, `active_profile_ref`, `on_invalid` and no weight/percentage field, so it is a profile switch, not a weighted canary.
- All five plugins handle `SetJumpOutcome::Rejected` locally as a log-and-continue (`canary/plugin.rs:172`, `dynamic_internal_upstream/plugin.rs:174`, `dynamic_external_upstream/plugin.rs:128`, `direct_endpoint/plugin.rs:159`, `region_route/plugin.rs:286`), confirming silent first-setter-wins conflicts.
- No intentional-design page justifies separate skeletons; value is maintenance/naming clarity, correctly rated P3.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P3 unchanged)
- No file under `canary`, `dynamic_internal_upstream`, `dynamic_external_upstream`, `direct_endpoint` or `region_route` in `edgion-gateway/src/plugins/http/` changed since feature-08-30.
- `backendref-upstream-jump-plugin-scope.md` is unchanged. User docs added only `region-route.md` documentation and no consolidation.
