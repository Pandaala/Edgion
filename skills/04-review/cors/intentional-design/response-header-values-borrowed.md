---
name: response-header-values-borrowed
description: Use when reviewing how the EdgionPlugins CORS plugin prepares response-header values, or when proposing precomputed runtime caches for them.
knowledge_type: intentional-design
---

# CORS response-header values borrow; no runtime cache fields

## Conclusion and scope

`Cors::build_cors_header_pairs` collects response headers as
`SmallVec<[(&'static str, Cow<'_, str>); 8]>`. Every value that is fixed once the
configuration is published borrows: `Access-Control-Allow-Methods`,
`Access-Control-Allow-Headers` (except the `**` reflection), `Access-Control-Expose-Headers`,
and the literal `true` flags. Request-dependent values own their bytes: the echoed origin, the
`**` reflection of `Access-Control-Request-Headers`, and `Timing-Allow-Origin`.
`Access-Control-Max-Age` is the one knowingly excluded case — see below.

`CorsConfig::get_allow_methods` returns `&str`, `CorsConfig::get_allow_headers` returns
`Cow<'_, str>` and takes `Option<String>` so the `**` reflection moves the request value
instead of copying it, and `CorsConfig::get_expose_headers` already returned `&str`.

`CorsConfig` carries **no** precomputed header cache fields. Its only `#[serde(skip)]` +
`#[schemars(skip)]` runtime fields remain the origin matchers (`origins_cache`,
`compiled_origins_regex`, `compiled_timing_regex`).

This covers EdgionPlugins CORS only. Native Gateway API `HTTPCORSFilter`
(`gateway_api_filters/cors_filter.rs`) has its own header builder and is out of scope.

## Rationale and evidence

`build_cors_header_pairs` runs on the request path for both callers —
`set_cors_headers_via_session` (simple requests and `preflightContinue`) and
`build_preflight_response` (terminating 204 preflight). The previous
`SmallVec<[(&'static str, String); 8]>` forced an owned value per entry, so the
getters cloned configuration strings on every cross-origin response. Per the
`CLAUDE.md` rule that data-plane performance outranks convenience, the container
type was the actual origin of the cost; the getter `clone()` calls were its symptom.

The three values named by the task are borrowable without any cache because:

- The `allow_methods: "**"` expansion is a fixed wire string, so it is the
  `FORCE_WILDCARD_METHODS` compile-time constant, not runtime state.
- Every other `allow_methods` / `allow_headers` / `expose_headers` value **is** the stored
  configuration string, so a cache field would hold a second copy of data the struct
  already owns.

This argument is specific to those three fields. It does not generalize to a CORS value
that is *derived* from configuration rather than stored as its wire form — see the
`Access-Control-Max-Age` limit below.

No wire behavior changed: header order, casing, the distinct `*` / `**` meanings,
credential restrictions, preflight-only emission of methods/headers, and the
deny-safe `validate_and_init` failure path are all preserved.

## Alternatives and limits

Rejected: adding `#[serde(skip)] + #[schemars(skip)]` precomputed header caches to
`CorsConfig`, as originally drafted in the `precompute-cors-response-headers` task.
The caches would duplicate fields the struct already holds, enlarge every
`CorsConfig::clone()` (which `Cors::new` performs per plugin construction), and add a
consistency obligation to the deny-safe branch of `validate_and_init` where
initialization failed. Borrowing achieves the same request-path result with less state.

Also rejected: precomputing `http::HeaderValue` instead of `&str`. It would make the
final header insertion a refcounted clone, but requires a mixed static/dynamic value
representation for a small additional gain.

Knowingly not addressed: `Access-Control-Max-Age`. `max_age` is an `Option<u32>` stored as a
number, so `max_age.to_string()` re-formats a *derived* static value on every preflight. It is
the one header here that a cache field would genuinely not duplicate. It was left allocating
because it falls outside the closing task's stated scope (`allowMethods`, `allowHeaders`,
`exposeHeaders`), it is emitted on preflight responses only rather than on every cross-origin
response, and a runtime cache field carrying a single formatted integer was not judged to earn
its consistency obligation on the deny-safe `validate_and_init` branch. A future change that
adds any other derived CORS wire value should revisit this together with it, rather than
treating this paragraph as a rule that derived values must not be cached.

Accepted limits: the conversion from `&str` to a header value inside
`insert_header` / `set_response_header` still copies bytes — that is inherent to those
APIs and is not claimed to be removed. `Cow<'_, str>` is wider than `String`, so the
inline `SmallVec` array is larger; it still holds all eight entries inline
(asserted by `maximum_header_set_stays_inline_and_preserves_order`).

## Verification and re-evaluation

- `cargo test -p edgion-resources --lib resources::edgion_plugins::plugin_configs::cors`:
  33 passed, including `test_methods_expansion` (`**`) and `test_headers_reflection`
  (`**` request reflection).
- `cargo test -p edgion-gateway --lib cors`: 36 passed, including
  `maximum_header_set_stays_inline_and_preserves_order`, which pins the full eight-header
  ordered wire values and the no-spill property.
- `cargo fmt --all`, `cargo check --workspace --all-targets`,
  `cargo clippy --workspace --all-targets`: passed.
- Agent-doc, SSA force, metrics inventory, and Pingora source policy guards: passed.
- No struct field was added or removed, so the CRD and conf_sync boundaries are
  untouched and `crd_coverage` / `plugin_crd_type_coverage` are unaffected.

Re-evaluate if the header-pair container changes owner, if another derived (rather than
stored) CORS wire value is added — which would make a real cache worth its cost and should
be decided together with `Access-Control-Max-Age` — or if the session / `ResponseHeader`
APIs gain a zero-copy header-value path.

## Resolution references

- `precompute-cors-response-headers`: refactor task created 2026-09-20, deferred from the
  CORS token-list validation fix; closed 2026-09-22 and its task file removed on close.
  Implemented here with the cache-field requirement deliberately not adopted; see the
  rejected alternatives above. Its validation/initialization-parity requirement was not
  delivered and is tracked separately in
  [merged-regex validation parity](../../../../tasks/todo/align-cors-merged-regex-validation-parity.md).
- Supersedes the "no request-path optimization is claimed" caveat previously carried in
  [token-list-validation](../regression-guards/token-list-validation.md).
