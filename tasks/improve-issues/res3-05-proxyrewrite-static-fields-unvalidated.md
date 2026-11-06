# res3-05: ProxyRewrite accepts invalid static `host`, `uri` and header names/values; at runtime the rewrite is skipped and the request is proxied unchanged

- **Category**: design-error
- **Severity**: P3
- **Scope**: local
- **Location**:
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/proxy_rewrite.rs:537-590` (`prepare`)
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/proxy_rewrite.rs:649-653` (`PluginValidation`)
  - `edgion-gateway/src/plugins/http/proxy_rewrite/plugin.rs:240-247,283-290,326-377` (runtime: `push_err` and continue)
  - Compare `edgion-resources/src/resources/header_modifier.rs:16-58` and `plugin_configs/response_rewrite.rs:120-188`

## Summary
`ProxyRewriteConfig::prepare()` is the whole admission check. It validates host/hostFromHeader
exclusivity, the `regexUri` regex, the JSON-patch plan and the `hostFromHeader` regex. It does not
check:
- `headers.{add,set}[].name` / `.value`, or `headers.remove[]` names, for HTTP token or value syntax;
- the static `host` value (authority syntax);
- the static `uri` template (it must render to an origin-form path).
The Controller therefore reports `Accepted=True`. On the Gateway each failing mutation logs
`header add fail` / `host rewrite fail` / `uri rewrite fail` to the plugin log and continues. The
request reaches the upstream *without* the rewrite the operator configured.

## Current behavior (evidence)
- `prepare()` (`proxy_rewrite.rs:537-590`) never touches `HeaderEntry.name/value`, `remove`,
  `host` or `uri`. `check_fields()` is `prepare().err()`.
- Runtime `modify_headers` (`plugin.rs:326-377`): `append_request_header`/`set_request_header`
  errors become `plugin_log.push_err(...)`, and processing goes on. `rewrite_host` (`:283-290`) and
  `rewrite_uri` (`:240-247`) do the same.
- Sibling surfaces validate these fields at admission:
  - `HTTPHeaderFilter::validate` (`header_modifier.rs:16-58`) checks token names, value bytes,
    lengths and set/add/remove overlap.
  - `ResponseRewriteConfig::check_fields` (`response_rewrite.rs:120-188`) checks names and values.
  - `DynamicExternalUpstream.overrideHost` checks `HeaderValue` and `is_valid_host_authority`
    (`dynamic_external_upstream.rs:221-231`).

## Impact
- A `headers.remove` entry meant to strip a client-supplied identity or internal header does
  nothing if its name is invalid (for example, it contains a space or a colon). Status still says it
  is accepted.
- An invalid static `host` sends the request upstream with the client's Host. For virtual-hosted
  upstreams this routes to a different tenant or site.
- Operators have to find these failures in per-request access-log plugin entries.

## Suggested direction
In `prepare()`, reuse the existing helpers:
- `is_valid_header_token` plus the `HEADER_NAME_MAX_LEN` check for every `add`/`set`/`remove` name;
- `HeaderValue::from_str` for literal values (skip only when the value contains `$`-variables);
- `is_valid_host_authority` + `HeaderValue` for static `host`;
- for a static `uri` with no variables, a parse as origin-form `http::uri::PathAndQuery`.
This keeps Controller and Gateway admission identical, as the project requires for plugin preparse.

## Notes
- `skills/04-review/http-body/intentional-design/plugin-authored-request-framing-is-trusted.md`
  accepts that operators may set framing headers on purpose. This finding is about *syntactically
  invalid* configuration being accepted and then silently ignored, not about deny-listing names.
- `skills/04-review/plugin-runtime/regression-guards/proxy-rewrite-path-token-boundaries.md` covers
  `$`-variable expansion in URIs, not static-field admission.

## Verification (independent)

Verdict: CONFIRMED. Severity P3 kept.
- `prepare()` (`proxy_rewrite.rs:537-590`) checks exclusivity, regexUri, jsonBody and hostFromHeader only; `PluginValidation` (`:649-653`) delegates to it.
- Runtime `rewrite_host`/`rewrite_uri`/`modify_headers` (`edgion-gateway/src/plugins/http/proxy_rewrite/plugin.rs:240-290,326-377`) `push_err` and continue, so the request is proxied without the configured rewrite.
- Correction: the `headers.remove` example has little real impact, because a header whose name is not a valid token cannot be present in a parsed request anyway. The meaningful cases are an invalid static `host` (client Host forwarded) and invalid `add`/`set` names/values silently dropped.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3
- `plugin_configs/proxy_rewrite.rs` and `edgion-gateway/src/plugins/http/proxy_rewrite/` are unchanged; `prepare()` still skips header/host/uri static validation.
