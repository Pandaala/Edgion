# ctrl3-01: Controller re-runs every WAF profile/optional-bundle compile on each EdgionPlugins reconcile, inline on the single EdgionPlugins worker

- **Category**: design-error
- **Severity**: P2
- **Scope**: local
- **Location**:
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs:343` (`parse` → `resolve_waf_bundles`)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs:618-973` (`resolve_waf_bundles`: loop over **all** profiles `:768`, `compile_trial` per required set `:884` and per optional bundle `:907`, aggregate cap 64 `:869`)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs:423-428` (`update_status` → `waf.validation_error()` → one more full compile)
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/waf.rs:1068-1230` (`compile_resolved_candidate` → `compile_rule_bundles_with_selection`), `:1352`, `:1391-1393` (`validation_error` = `compile_generation().err()`)
  - `edgion-controller/src/services/credential_manager/service.rs:744`, `:944-960` (`notify_consumers` requeues every consumer EdgionPlugins on each token publication)

## Summary

For an `EdgionPlugins` with `spec.waf`, every reconcile of that resource strictly compiles the
WAF candidate for **every** profile in the referenced `WafPolicy`, once more per optional bundle,
and once more in `update_status` — up to 64 + 1 full SecLang/CRS compilations — synchronously
inside the async `parse` of the single EdgionPlugins worker. Nothing is cached by content
identity, so the same CRS bundle is recompiled on every requeue regardless of what changed. A
bundle/policy edit, any referenced Secret/ConfigData change, or an OAuth2 token refresh of a
co-located `CredentialInjector` therefore stalls the whole EdgionPlugins queue (every auth,
rate-limit and AI plugin change for the cluster) behind repeated CPU-heavy compiles, and pins a
Tokio worker thread while doing so.

## Current behavior (evidence)

- `EdgionPluginsHandler::parse` (`edgion_plugins.rs:237`) calls `resolve_waf_bundles` unconditionally
  (`:343`). It clears all previous resolution state (`:622-635`) and recomputes from scratch.
- `resolve_waf_bundles` iterates `policy.profiles.keys()` (`:768`), i.e. all profiles, not just the
  desired/default one. For each profile it calls `compile_trial` for the required set (`:884`) and
  then once per optional bundle, each time recompiling required + accumulated optional bundles
  (`:901-916`). Only `aggregate_trials > 64` stops it (`:869`).
- Each trial is `WafConfig::compile_resolved_candidate` → `compile_rule_bundles_with_selection`
  (`waf.rs:1217`), the full frontend + semantic compile that the Gateway also runs. The result is
  discarded except for `selection_summary`.
- `update_status` then calls `waf.validation_error()` (`edgion_plugins.rs:425`), which is
  `self.compile_generation().err()` (`waf.rs:1391-1393`) — another full compile of the selected
  profile in the same reconcile.
- No `spawn_blocking`/`block_in_place` exists anywhere under `conf_mgr/` except FS IO
  (`grep -rn spawn_blocking conf_mgr/`), so the compiles run on a Tokio worker inside the one
  serial EdgionPlugins worker.
- Requeue sources that trigger the full recompile without any WAF input change:
  `requeue_edgion_config_data_referrers` (any referenced IpList/Selector/ConfigData,
  `edgion_config_data.rs:149-160`), Secret changes via the secret ref manager, LinkSys credential
  results, and `notify_consumers` in the OAuth2 credential manager on every token publication
  (`service.rs:744`, `:956-957`).
- The shipped CRS projection is ~1 MB / 565 units (`waf-06`); the Gateway exposes
  `edgion_gateway_waf_compile_microseconds_total`, but the Controller has no equivalent metric,
  so the cost is invisible.

## Industry comparison

- Coraza / ModSecurity integrations (Coraza-proxy-wasm, APISIX coraza plugin, Envoy + Coraza)
  compile a rule set once per configuration identity and reuse it; validation in control planes
  (e.g. Istio `istioctl analyze`, Envoy Gateway translation) is keyed on the input generation and
  is not redone for unrelated reconciles.
- Kubernetes controller-runtime guidance: reconcilers must stay cheap and idempotent; expensive
  derived artifacts are cached by input hash (e.g. Envoy Gateway caches translated xDS per input
  revision).

## Impact

- Head-of-line blocking on the single EdgionPlugins worker: one CRS bundle edit referenced by N
  plugin resources costs N × (profiles + optional bundles + 1) full compiles serialized on one
  queue; credential rotations, auth changes and rate-limit edits on unrelated EdgionPlugins wait
  behind them. This violates the intent of workqueue rule 5 ("no latency in handler processing").
- A Tokio worker thread is blocked for the whole compile series, reducing Admin API / conf_sync
  responsiveness on small controller pods.
- OAuth2 token refreshes (every few minutes for short-lived tokens) silently cause periodic full
  WAF recompiles for any EdgionPlugins that combines CredentialInjector with WAF.

## Suggested direction

1. Introduce a content-addressed verdict cache in the Controller keyed by
   `(policy digest, profile, ordered bundle digests, selection digest, request-body mode)` storing
   `{ok | error, selection_summary}`. Reconciles with unchanged inputs become hash lookups; this
   also lets N plugins sharing a policy validate once.
2. Only trial-compile the profiles that can be selected now (desired + default); validate other
   profiles lazily when a Selector switches to them (the Selector change already requeues).
3. Drop the second compile in `update_status` by carrying the verdict from `parse`.
4. Add a Controller-side compile duration metric mirroring the Gateway one.

Recommendation: (1)+(3) first — they remove nearly all repeated work without changing semantics.

## Notes

- Checked `skills/04-review/waf/regression-guards/named-profile-selection-and-bundle-publication.md`
  and `static-tag-selection-before-compilation.md`: they require strict Controller compilation of
  candidate profiles and bounded trials, but do not address recompiling identical inputs or the
  worker-latency rule; caching the verdict preserves their semantics.
- `waf-06` covers the Gateway-side per-resource generations and mentions "trial-compiled again in
  the Controller" in passing; this finding is the Controller worker/head-of-line consequence.
- `worker-path-status-io-inline-accepted.md` accepts inline status IO only; it does not cover
  CPU-bound compilation in handlers.

## Verification (independent)

- **Verdict**: CONFIRMED (P2)
- `edgion_plugins.rs:343` calls `resolve_waf_bundles` unconditionally in `parse`; `:768` iterates `policy.profiles.keys()` (all profiles), and the `compile_trial` closure (`:866-884`) runs once for the required set plus once per optional bundle (`:901-916`), capped only by `aggregate_trials > 64`.
- `update_status` (`edgion_plugins.rs:423-426`) calls `waf.validation_error()`, which is `self.compile_generation().err()` (`waf.rs:1391-1393`) - one more full compile per reconcile.
- No content-keyed cache exists in the handler, and the only `spawn_blocking` uses under `conf_mgr/` are FS IO (`file_system/*.rs`), so compiles run synchronously on a Tokio worker inside the single EdgionPlugins worker.
- `credential_manager/service.rs:944-960` `notify_consumers` requeues every consumer EdgionPlugins on token publication, confirming a recurring non-WAF trigger for the full recompile.
- Not a duplicate of `waf-06` (Gateway-side per-resource generations); this is the Controller worker head-of-line consequence.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2
- `git diff feature-08-30..HEAD` shows no change to `handlers/edgion_plugins.rs`, `plugin_configs/waf.rs` or `services/credential_manager/`; cited line numbers are current.
- `edgion_plugins.rs:343` still calls `resolve_waf_bundles(&mut ep, ctx)` unconditionally; no verdict cache was added.
- `skills/04-review/waf/SKILL.md` changed only its task list (dropped optimization tasks); no new accepted-design page covers controller recompilation.
