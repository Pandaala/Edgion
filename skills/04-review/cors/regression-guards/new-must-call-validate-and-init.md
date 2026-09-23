---
name: new-must-call-validate-and-init
description: Use when reviewing findings that the CORS plugin constructor clones the config without compiling origins_cache / compiled_origins_regex / compiled_timing_regex, leaving allow_origins_by_regex, comma-separated lists, and *.domain subdomain wildcards silently ignored; documents that this was fixed (fr-types-02), mirroring the IpRestriction fr-types-01 fix.
knowledge_type: regression-guards
---

# CORS `Cors::new` Compiles Origin Matchers — FIXED

## Conclusion

`Cors::new` (`edgion-gateway/src/plugins/http/cors/plugin.rs`) now clones the config and calls
`config.validate_and_init()` before wrapping it in the `Arc`, so `origins_cache`,
`compiled_origins_regex`, and `compiled_timing_regex` are populated on the live request-filter
path. The runtime's cached preflight Arc (`PluginRuntime::add_from_request_filters`,
`edgion-gateway/src/plugins/runtime/plugin_runtime.rs`) likewise runs `validate_and_init()` on the
`CorsConfig` before caching it. Since `change-fix-01`, the global preflight handler uses only the
cache's presence to delegate to the normal route runtime; it no longer constructs an ad-hoc
`Cors::from_arc`. Fixed by `fr-types-02` (2026-05-29), with ownership updated by
`change-fix-01` (2026-08-02).

Fix suggestions of the form "`Cors::new` never calls `validate_and_init`, so
`compiled_origins_regex` / `origins_cache` stay `None` → `allow_origins_by_regex`,
comma-separated `allow_origins`, and `*.domain` subdomain wildcards are silently ignored
(multi-origin/regex CORS broken)" are **already done** — do not re-report.

## Core Rationale

**1. The defect was constructor-only, not the evaluator**

`origins_cache` / `compiled_origins_regex` / `compiled_timing_regex` carry `#[serde(skip)]`, so
they are `None` after the gRPC/YAML → preparse → runtime path. Only `CorsConfig::validate_and_init`
(`cors.rs`) builds them; its sole caller `CorsConfig::new` is unused by the runtime. The old
`Cors::new` did `Arc::new(config.clone())` with no init call, so `is_origin_allowed` fell into the
single-origin `else` branch (raw `allow_origins == origin` string compare, which never matches a
comma-separated string) and skipped the regex branch entirely. At the time,
`validation_error()` at preparse only re-compiled each regex pattern for *syntax* (and did not
build the `#[serde(skip)]` fields), so `seal_fail_closed` never fired for a
syntactically valid config. The current validator also checks the merged expression.

**2. The route runtime is the current construction path**

The per-route request filter is built via `Cors::new` (`create_request_filter_from_edgion`) and
owns both normal requests and preflights. The global OPTIONS preflight handler checks the runtime's
cached `Arc<CorsConfig>` only to detect this ownership and delegate. `Cors::from_arc` still assumes
an already-initialized config, but it is no longer a separate request-path construction site.

**3. Input rejection and matcher-construction failures are separate boundaries**

For a freshly deserialized config, a missing compiled matcher cannot match an origin through
that matcher. This is the deny-direction consequence covered by the historical constructor fix;
it is not a proof that every initialization error disables the whole CORS configuration.
On `validate_and_init` error both construction sites retain the config and emit `tracing::warn!`.
Historically, a merged-regex failure could occur after the origins cache was populated. The
current validator rejects that error before initialization mutates cached fields; raw exact
matching remains independent of compiled fields.

Construction runs at plugin build (config-apply / hot-reload), off the request path, so this
tracing does not violate the data-plane no-tracing rule. The shared `validation_error()` rejects
credential-policy violations, invalid method/header tokens, and regex lists whose wrapped/merged
runtime expression cannot compile. Both validation and initialization use
`origin_regex_patterns` and `compile_merged_patterns`, including the HTTPS-only `*.domain`
shorthand and the separate timing-origin matcher. A second list item reusing the first item's
named capture group is rejected at ingest with the field and one-based item number; the
diagnostic does not echo operator regex content. The Gateway construction fallback remains for
unexpected failures. This closes
`align-cors-merged-regex-validation-parity.md`; the earlier `precompute-cors-response-headers`
task delivered request-path header preparation separately (see
[response-header values borrow](../intentional-design/response-header-values-borrowed.md)).

**4. Regression locked at the constructor→evaluator boundary**

Unit tests in `cors/plugin.rs` (`test_new_compiles_regex_origin_allowed`,
`test_new_compiles_comma_separated_list`, `test_new_compiles_subdomain_wildcard`) build the config
the way deserialization yields it (compiled fields `None`, **not** routed through
`CorsConfig::new`), call `Cors::new`, and assert the origin is allowed. A test that constructs via
`CorsConfig::new` would pre-build the matchers and mask the bug — re-reports must verify the
compiled-fields-`None` path. The integration suite adds a `cors-regex` config (regex +
`*.sub` wildcard + comma-separated list) exercising the request-filter path end-to-end.

## Fix Suggestions Not Accepted

- "CORS multi-origin / regex / `*.domain` is silently broken / `Cors::new` missing
  `validate_and_init`" — already fixed; verify the current `Cors::new` clones + calls
  `validate_and_init` and that the `plugin_runtime.rs` cache site does the same before filing.
- "Make `from_arc` call `validate_and_init`" — not accepted; callers must supply an initialized
  config, while the production route runtime uses `Cors::new` as its validated constructor.

## Re-evaluation Triggers

Re-open only if:

- A new `#[serde(skip)]` runtime field is added to `CorsConfig` and either construction site is not
  updated to populate it.
- `validate_and_init` and `validation_error` diverge such that an operator-reachable config passes
  preparse but fails matcher build at construction. Assess the remaining initialized and raw
  matching fields before describing any construction fallback as deny-safe.

## Reference Cases

- `fr-types-02` — original finding, fixed 2026-05-29 (this file).
- Sibling pattern: [`../ip-restriction/http-create-must-call-validate-and-init.md`](../../ip-restriction/regression-guards/http-create-must-call-validate-and-init.md)
  (HTTP `IpRestriction::create`, fr-types-01 — same `#[serde(skip)]` + constructor-skips-init class).
- Related CORS credential guard: [`regex-origins-with-credentials-rejected.md`](regex-origins-with-credentials-rejected.md)
  (fr-auth2-01 — why `allow_origins_by_regex` + credentials is rejected at preparse).
- Source: `edgion-gateway/src/plugins/http/cors/plugin.rs` (`Cors::new`),
  `edgion-gateway/src/plugins/runtime/plugin_runtime.rs` (`add_from_request_filters` CORS cache),
  `edgion-resources/src/resources/edgion_plugins/plugin_configs/cors.rs` (`validate_and_init`, `is_origin_allowed`).
