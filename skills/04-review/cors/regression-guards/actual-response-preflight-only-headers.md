---
name: actual-response-preflight-only-headers
description: Use when reviewing which CORS response headers belong on an actual (non-preflight) response; Access-Control-Allow-Methods / -Allow-Headers are preflight-only in the EdgionPlugins CORS plugin.
status: fixed
finding_id: gw-plugin-acl-rl-03
closed: 2026-08-03
knowledge_type: regression-guards
---

# CORS Actual Responses Carry No Preflight-Only Headers — FIXED

## Conclusion

In the `EdgionPlugins` CORS plugin (`edgion-gateway/src/plugins/http/cors/plugin.rs`),
`build_cors_header_pairs` emits `Access-Control-Allow-Methods` and
`Access-Control-Allow-Headers` only when `is_preflight == true`, alongside the pre-existing
`Access-Control-Max-Age` gate. An actual (non-preflight) cross-origin response carries only
`Access-Control-Allow-Origin`, `Access-Control-Allow-Credentials`,
`Access-Control-Expose-Headers`, plus the independent `Timing-Allow-Origin` and
`Access-Control-Allow-Private-Network`.

Re-reports claiming the two headers are missing from an actual response are **not accepted**;
this is the specified behavior, not a regression.

## Core Rationale

**1. The Fetch standard splits the two response header sets**

Per WHATWG Fetch 3.2.2/3.2.3, `Access-Control-Allow-Methods` and `Access-Control-Allow-Headers`
are read exclusively by the CORS-preflight fetch algorithm, which decides whether the actual
request may proceed. The CORS check applied to an actual-request response inspects only
`Access-Control-Allow-Origin`, `Access-Control-Allow-Credentials` and
`Access-Control-Expose-Headers`. A browser ignores the two preflight-only headers on an actual
response, so emitting them was never a correctness or security matter — it was unconditional
per-response overhead on a data-plane hot path.

**2. The `**` shorthands made the waste concrete**

With `allowHeaders: "**"` an actual request carries no `Access-Control-Request-Headers`, so
`CorsConfig::get_allow_headers(None)` fell back to the literal `"*"` and every actual response
gained a content-free `Access-Control-Allow-Headers: *`. With `allowMethods: "**"` the fully
expanded nine-verb list was rendered per response. The gate also removes the
`access-control-request-headers` lookup from the actual-request path.

**3. The plugin already modeled the split**

The `is_preflight` parameter existed solely to gate `Access-Control-Max-Age`; the two header
pushes above it were simply not covered. Envoy's CORS filter draws the same line — preflight
responses get Allow-Methods/Allow-Headers/Max-Age, actual responses do not.

**4. `preflightContinue` is unaffected**

`handle_preflight` passes `is_preflight = true` on both branches, so a forwarded preflight
(`preflightContinue: true`) still queues the full preflight header set for the upstream response
stage. Only `handle_normal_request` passes `false`.

## Scope: the Gateway API `HTTPCORSFilter` is deliberately excluded

`edgion-gateway/src/plugins/runtime/gateway_api_filters/cors_filter.rs` has the same shape
(`build_cors_headers(.., is_preflight)` gates only `Access-Control-Max-Age`) and was **not**
changed. The Gateway API v1.5 `HTTPCORSFilter.allowMethods` schema states that "when the
`AllowMethods` field is configured with one or more methods, the gateway must return the
`Access-Control-Allow-Methods` response header" without qualifying it to preflight, and upstream
conformance may assert on that wording. Aligning the Gateway API filter is an upstream-spec
judgement, not part of this Edgion-plugin fix. A finding against `cors_filter.rs` on this pattern
is a separate decision, not a re-report of `gw-plugin-acl-rl-03`.

## Fix Suggestions Not Accepted

- Re-adding `Access-Control-Allow-Methods` / `-Allow-Headers` to actual responses "for API
  consumers or debugging tooling" — browsers ignore them and the access log already records the
  CORS decision (`cors actual` / `origin match:allow`).
- Merging the two `if is_preflight` blocks into one — it would reorder `Access-Control-Max-Age`
  ahead of `Access-Control-Expose-Headers` for no benefit.
- Applying the same gate to `cors_filter.rs` as part of this finding — see the scope section.
- Gating `Access-Control-Expose-Headers` or `Timing-Allow-Origin` the same way — both are
  actual-response headers.

## Re-evaluation Triggers

Re-open this decision only if:

- The Fetch standard changes which headers the actual-request CORS check reads.
- Gateway API GEP-1767 mandates the preflight-only headers on actual responses and Edgion
  chooses to unify the two CORS implementations under that rule.

## Reference Cases

- Finding `gw-plugin-acl-rl-03` — fixed 2026-08-03.
- Regression tests: `test_actual_response_omits_preflight_only_headers`,
  `test_actual_response_omits_wildcard_shorthand_headers`,
  `test_preflight_response_keeps_methods_and_headers`
  (`edgion-gateway/src/plugins/http/cors/plugin.rs`).
- Integration suite: `cors_simple_matching_origin`, `cors_preflight_matching_origin`
  (`edgion-tests/src/client/suites/edgion_plugins/cors/cors.rs`).
- [WHATWG Fetch — CORS protocol](https://fetch.spec.whatwg.org/#http-cors-protocol).
- [Envoy CORS filter](https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/cors_filter).
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/cors.rs` —
  `get_allow_methods` / `get_allow_headers`.

## Resolution references

- `gw-plugin-acl-rl-03` — 2026-08-03.
