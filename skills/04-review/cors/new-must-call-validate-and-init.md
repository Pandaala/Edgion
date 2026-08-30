---
name: new-must-call-validate-and-init
description: Use when reviewing findings that the CORS plugin constructor clones the config without compiling origins_cache / compiled_origins_regex / compiled_timing_regex, leaving allow_origins_by_regex, comma-separated lists, and *.domain subdomain wildcards silently ignored; documents that this was fixed (fr-types-02), mirroring the IpRestriction fr-types-01 fix.
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
comma-separated string) and skipped the regex branch entirely. `validation_error()` at preparse
only re-compiles each regex pattern for *syntax* (it does not build the `#[serde(skip)]` fields),
so `seal_fail_closed` never fired for a syntactically-valid config.

**2. The route runtime is the current construction path**

The per-route request filter is built via `Cors::new` (`create_request_filter_from_edgion`) and
owns both normal requests and preflights. The global OPTIONS preflight handler checks the runtime's
cached `Arc<CorsConfig>` only to detect this ownership and delegate. `Cors::from_arc` still assumes
an already-initialized config, but it is no longer a separate request-path construction site.

**3. Failure is deny-safe, not fail-open**

CORS fails in the DENY direction: an uncompiled matcher rejects origins that should be allowed
(no security regression — attackers cannot bypass CORS). On `validate_and_init` error both sites
keep the uncompiled config (regex/cache origins skipped) and emit `tracing::warn!`. The constructor
runs at plugin build (config-apply / hot-reload), off the request path, so `tracing` is permitted
here — identical to `IpRestriction::create` and the stream constructors; it does not violate the
data-plane no-tracing rule. In practice the error path is unreachable for an operator config
because `validation_error()` (run before construction in `add_from_request_filters`) already
rejects the same security cases (credentials + wildcard/regex) and regex syntax errors.

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
  preparse but fails matcher build at construction (would now correctly fall back deny-safe, but the
  asymmetry should be reviewed).

## Reference Cases

- `fr-types-02` — original finding, fixed 2026-05-29 (this file).
- Sibling pattern: [`../ip-restriction/http-create-must-call-validate-and-init.md`](../ip-restriction/http-create-must-call-validate-and-init.md)
  (HTTP `IpRestriction::create`, fr-types-01 — same `#[serde(skip)]` + constructor-skips-init class).
- Related CORS credential guard: [`regex-origins-with-credentials-rejected.md`](./regex-origins-with-credentials-rejected.md)
  (fr-auth2-01 — why `allow_origins_by_regex` + credentials is rejected at preparse).
- Source: `edgion-gateway/src/plugins/http/cors/plugin.rs` (`Cors::new`),
  `edgion-gateway/src/plugins/runtime/plugin_runtime.rs` (`add_from_request_filters` CORS cache),
  `edgion-resources/src/resources/edgion_plugins/plugin_configs/cors.rs` (`validate_and_init`, `is_origin_allowed`).
