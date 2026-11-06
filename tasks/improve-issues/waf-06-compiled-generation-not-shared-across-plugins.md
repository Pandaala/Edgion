# waf-06: Each EdgionPlugins ships ~1 MB of resolved CRS text and compiles its own WAF generation; nothing is shared by content digest

- **Category**: design-error
- **Severity**: P2
- **Scope**: local
- **Location**:
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/waf.rs:262-360` (`WafConfig.resolved_policy` / `resolved_bundles`: "Complete uncompiled bundle contents, resolved by the Controller")
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs:969` (bundles embedded per EdgionPlugins)
  - `edgion-gateway/src/plugins/http/plugin_generation.rs:12-27` (entry cache lives inside one immutable `PluginGeneration`; "source replacement creates another owner")
  - `edgion-gateway/src/plugins/runtime/plugin_runtime.rs:413-444,461-490` (`add_waf_with_cache` → `WafPlugin::try_new` → `config.compile_generation()`)
  - `edgion-gateway/src/plugins/http/waf/mod.rs:101-140` (`try_new` compiles and owns a private `Arc<CompiledGeneration>`)

## Summary
The Controller copies the full, uncompiled rule bundles into every EdgionPlugins resource whose `Waf` entry
references a policy, and the Gateway compiles a separate `CompiledGeneration` for each such resource. The
supported CRS bundle alone is ~1 MB of SecLang (`generated/EdgionConfigData_crs-supported-core.yaml`), and
compilation builds hundreds of regex NFAs, phrase automata and libinjection tables. The generation cache is
scoped to one immutable EdgionPlugins source, so editing any unrelated entry (CORS, a rate-limit number) in
that resource recompiles CRS, and N routes/apps with their own EdgionPlugins hold N identical generations.

## Current behavior (evidence)
- `WafConfig.resolved_bundles: Option<Vec<ResolvedWafBundle>>` is serialized on conf_sync (redacted only in
  admin output); the Controller fills it per EdgionPlugins (edgion_plugins.rs:969).
- `PluginGeneration::new` creates a fresh `EntryCache` for each new source; `add_waf_with_cache` only reuses a
  compiled WAF within that cache (plugin_runtime.rs:425-431), i.e. only across policy re-evaluations of the
  same unchanged source.
- `WafPlugin::try_new` always calls `config.compile_generation()` and wraps the result in a new `Arc`
  (mod.rs:101-140); `edgion_gateway_waf_generation_bytes` sums every live generation independently.
- A WafRuleBundle/WafPolicy edit re-resolves all referencing EdgionPlugins, so every one of them is
  recompiled on every Gateway (and trial-compiled again in the Controller).

## Industry comparison
- Envoy ECDS / named filter configs let many listeners and routes share one filter instance; per-route
  overrides reference it rather than copying it.
- Coraza-based gateways (Traefik plugin, coraza-proxy-wasm) build one WAF per configured directive set and
  attach it to routes by reference; ModSecurity-nginx shares the rules object across locations that inherit
  the same `modsecurity_rules_file`.

## Impact
Gateway memory and reload CPU scale with the number of EdgionPlugins that enable WAF rather than with the
number of distinct rule sets; conf_sync payload grows by ~1 MB per WAF-enabled EdgionPlugins; a bundle edit
causes an N-fold recompile spike on every Gateway at the same moment.

## Suggested direction
Key the compiled generation by a content hash of everything compile-relevant (bundle digests already
pinned by the policy, selected profile/selection, inspection mode and the few options that change
compilation) and keep a process-wide `Weak`-valued map from key to `Arc<CompiledGeneration>`; `WafPlugin`
holds the shared `Arc` plus its per-plugin options. Optionally ship bundles once as their own conf_sync
resources and let EdgionPlugins carry only the resolved digests, so the Gateway resolves by reference.
Keep the existing "retain last known-good on invalid update" behavior per plugin.

## Notes
Checked `skills/04-review/waf/regression-guards/named-profile-selection-and-bundle-publication.md` (one
validated generation per publication — about atomicity, not sharing) and the Controller handler. No
intentional-design page discusses cross-resource generation sharing; not tracked under `tasks/`.

## Related findings

- gw-plugrt-02, gw-ext-09 (same per-entry rebuild root cause).

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P2 unchanged. It shares a root cause with gw-plugrt-02 and gw-ext-09 (other batches), but the WAF-specific cross-resource sharing and conf_sync payload aspects are unique.
- `plugins/http/plugin_generation.rs:12-27`: `EntryCache` lives inside one `PluginGeneration`, and source replacement creates a new owner.
- `plugin_runtime.rs:413-444` `add_waf_with_cache` reuses a compiled WAF only from that per-source cache. Otherwise it calls `add_waf`, which reaches `WafPlugin::try_new` → `config.compile_generation()` (`waf/mod.rs:103`).
- No process-wide, content-keyed generation map exists.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- `plugin_generation.rs` and the controller `edgion_plugins.rs` handler are unchanged; `plugin_runtime.rs` changed only one atomic-counter line (`:316`, `fetch_update`→`try_update`).
- `WafPlugin::try_new` (`waf/mod.rs:101-140`) still compiles a private generation; no content-keyed process-wide map exists.
- `git diff feature-08-30..HEAD` shows no change under `edgion-gateway/src/plugins/http/waf/`, `edgion-gateway/src/ctx.rs`, `edgion-gateway/src/request_body/config.rs`, or `edgion-waf/src/` apart from a `serde_yaml`→`yaml_serde` dependency rename and a `fetch_update`→`try_update` rename in `json_inspection.rs:12`; none touch this finding's code paths.
