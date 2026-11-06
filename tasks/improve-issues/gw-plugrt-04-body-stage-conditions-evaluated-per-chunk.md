# gw-plugrt-04: Response-body-stage conditions are re-evaluated per chunk (probability flips mid-response) for a stage that serves one plugin

- **Category**: design-error
- **Severity**: P3 (downgraded from P2 in independent verification)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/runtime/conditional_filter.rs:232-252` (`ConditionalUpstreamResponseBodyFilter::run_upstream_response_body_filter`); `edgion-gateway/src/plugins/runtime/conditions/evaluator.rs:1006-1030` (`ProbabilityCondition::evaluate`, `rand::random` at `:1024,1027`), `:202-207` (TimeRange); `edgion-resources/src/resources/edgion_plugins/validate.rs:450-480` (no stage restriction); `edgion-gateway/src/plugins/runtime/plugin_runtime.rs:2889-2950` (only `BandwidthLimit` is constructible in this stage), `:3500-3548`

## Summary
Entry `conditions` on `upstreamResponseBodyFilterPlugins` are evaluated on every body chunk. A
`probability` condition without a key draws a new random number per chunk, so `ratio: 0.1` on
`BandwidthLimit` throttles ~10% of chunks of every response instead of 10% of responses; a `timeRange`
boundary or an IP-list refresh can toggle the plugin in the middle of one download. Validation accepts these
conditions for the body stage. The whole stage (CRD array, trait, conditional wrapper, ExtensionRef body
path, entry-slot stage, policy masks) exists for a single configurable plugin.

## Current behavior (evidence)
- `ConditionalUpstreamResponseBodyFilter::run_upstream_response_body_filter` calls
  `conditions.evaluate(session)` for each chunk (`conditional_filter.rs:246`) and returns `None` (no delay)
  on Skip; no per-response memo exists.
- `ProbabilityCondition::evaluate` uses `rand::random::<f64>()` when no key is set or the key is missing
  (`evaluator.rs:1020-1028`).
- `validate_plugins_with_waf` compiles/validates conditions for `upstreamResponseBodyFilterPlugins` with the
  same rules as every other stage (`validate.rs:450-480`); nothing rejects probability/time conditions here.
- `create_upstream_response_body_filter_from_edgion` builds only `BandwidthLimit`
  (`plugin_runtime.rs:2895`); every other variant is `WrongStage`. Writable body work goes through the
  separate `StreamingResponseProcessor` driver (Wasm, ExtProc, AI, transcoder).

## Industry comparison
Envoy evaluates per-route/per-filter decisions once per stream (at headers) and filters keep per-stream
state; Kong/APISIX evaluate plugin applicability (`_meta.filter`, route match) once per request, and
`body_filter` runs for the plugins selected for that request.

## Impact
- Sampling semantics silently differ from the request and header stages; users who copy a canary-style
  `probability` condition onto BandwidthLimit get per-chunk randomness (uneven pacing, effective rate not
  what was configured), with no validation error.
- Predicate work (header lookups, regex, IP radix) is repeated per chunk on long downloads/SSE.

## Suggested direction
- Evaluate body-stage conditions once per response (at the first chunk or at response-header time) and
  cache the run/skip bit in the request context per entry; the per-chunk path reads the bit.
- Alternatively, reject non-deterministic conditions (`probability` without key, `timeRange`) for this
  stage in the shared validator.
- Consider collapsing the stage: make `BandwidthLimit` a response-stage plugin that installs a per-response
  pacing companion, removing the `upstreamResponseBodyFilterPlugins` array, its trait and wrapper.

## Notes
- `rate-limit/intentional-design/bandwidth-limit-per-stream.md` accepts per-stream pacing without a token
  bucket; it does not cover condition evaluation. `plugin-metadata/mandatory-design/condition-set-single-owner.md`
  requires the shared Condition engine — the fix keeps that engine and only changes evaluation frequency.
  Not tracked under `tasks/`.

## Verification (independent)

- **Verdict**: DOWNGRADED (P2 → P3)
- Claims hold: `edgion-gateway/src/plugins/runtime/conditional_filter.rs:236-252` evaluates conditions on every body chunk; `conditions/evaluator.rs:1020-1028` draws `rand::random` when no key (or missing key) is set; `plugin_runtime.rs:2895` shows `BandwidthLimit` is the only constructible plugin for this stage.
- Why lower: the blast radius is one plugin (`BandwidthLimit`) combined with an unusual condition (unkeyed `probability` or a `timeRange` boundary mid-download). The fix (memoize per response or reject non-deterministic conditions for this stage) is cheap and correct, but this is a narrow semantic edge, not a P2 gateway problem. Stage collapse is a separate cleanup suggestion.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P3 unchanged)
- `plugins/runtime/conditional_filter.rs`, `conditions/evaluator.rs` and `edgion_plugins/validate.rs` have no diff.
