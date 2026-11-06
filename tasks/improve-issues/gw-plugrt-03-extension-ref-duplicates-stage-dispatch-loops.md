# gw-plugrt-03: ExtensionRef re-implements every stage dispatch loop and has already diverged (no per-plugin latency for route-attached plugins)

- **Category**: over-design
- **Severity**: P2 (worth doing)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs:176-412` (`run_extension`, `run_prepared_extension_async`, `run_extension_body`); `edgion-gateway/src/plugins/runtime/plugin_runtime.rs:3246-3400` (`run_request_plugins_impl`), `:3416-3478` (response sync), `:3500-3548` (body), async response runner; `:3318-3321`, `:3458-3461`, `:3626-3629` (time_cost skipped for `refer_to`)

## Summary
Route-attached `EdgionPlugins` execute only through `ExtensionRefFilter` (the one attachment form for
routes). Instead of delegating to `PluginRuntime`'s stage runners, `ExtensionRefFilter` contains its own copy
of the request, sync-response, async-response and body loops (identity install, dye plan install, anti-spoof
strip, log allocation, early exit, outcome frames). The copies have already drifted: plugins run through
an ExtensionRef never get `time_cost`, so per-plugin latency is missing for the main attachment path.

## Current behavior (evidence)
- Outer runner: `run_request_plugins_impl` measures each filter and sets `plugin_log.time_cost` — but skips
  it when `refer_to` is set, i.e. for the ExtensionRef filter itself (`plugin_runtime.rs:3318-3321`; same at
  `:3458-3461` and `:3626-3629`).
- Inner runner: `run_prepared_extension_async` (`extension_ref.rs:271-378`) and `run_extension`
  (`:176-238`) create `inner_log` and push it with `push_to_edgion_plugins_log`, never setting `time_cost`.
  `time_cost` is assigned nowhere else (`grep time_cost` over `edgion-gateway/src`). Result: neither the
  ExtensionRef entry nor any plugin inside it carries latency, while global/Gateway-level plugins do.
- Other divergences between the two copies: IP-dependency unavailability returns a bodiless 500 in the inner
  copy (`extension_ref.rs:197-202, 277-282`) versus the outer `RejectFilter` JSON body; the inner copies
  re-check `rate_limit_local_outcome_slots` framing themselves (`:287-292, 318-325`); every comment in the
  inner loops ("mirrors `PluginRuntime::run_...`") documents a manual synchronization obligation across
  4 stages × 2 places.

## Industry comparison
Envoy runs one `FilterManager` iteration over the chain; per-route configuration is looked up by each
filter, not by a nested chain executor. Kong/APISIX run one phase loop over the merged plugin list.
A referenced bundle is a list-splicing concern, not a second runtime.

## Impact
- Operators cannot see which route-attached plugin is slow (auth call, ExtProc, DSL) from the access log —
  the field exists and works only for global/gateway plugins.
- Every future change to dispatch (new result variant, new per-entry hook, new observability) must be made
  in two places per stage; the drift above shows this already fails.

## Suggested direction
Extract one stage executor per stage on `PluginRuntime` (e.g. `run_request_chain(&self, session:
&mut dyn PluginSession, sink: &mut impl LogSink)`) that owns identity/dye/strip/log/time/early-exit, and
have both the top-level runners and `ExtensionRefFilter` call it, differing only in the log sink (stage log
vs. EdgionPlugins log token). Longer term, consider splicing referenced runtimes into the per-scope route
chain at publication time so the request path has no nested chain at all.

## Notes
- Checked `plugin-runtime/intentional-design/plugin-runtime-god-match.md` and
  `add-from-builders-duplication.md`: both concern cold-path construction matches/builders, not the
  hot-path dispatch loops. `extension-ref-tokenized-plugin-log-contract.md` defines the log token contract but
  says nothing about latency. Not tracked under `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED (severity P2 unchanged)
- `edgion-gateway/src/plugins/runtime/plugin_runtime.rs:3318-3321,3458-3461,3626-3629`: outer runners skip `time_cost` when `refer_to` is set.
- `time_cost` is assigned only at those three sites (`grep time_cost edgion-gateway/src`), so plugins executed inside `ExtensionRefFilter` (`gateway_api_filters/extension_ref.rs:176-412`) never record latency, although the access-log doc (`docs/en/ops-guide/observability/access-log.md:331`) presents `logs[].time_cost` as per-plugin execution time.
- No review page accepts the duplicated inner dispatch loops (`plugin-runtime-god-match.md`/`add-from-builders-duplication.md` cover cold-path builders only).

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `gateway_api_filters/extension_ref.rs` has no diff; `plugin_runtime.rs` has only a one-line `try_update` rename at `:316`, so the cited `time_cost` skip sites keep their line numbers.
