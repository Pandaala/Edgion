---
name: httproute-external-auth-runtime-fail-closed
description: Use when reviewing HTTPRoute-native ExternalAuth construction failures and invalid plugin sealing.
status: fixed
finding_id: gw-plugin-runtime-01
closed: 2026-08-02
---

# HTTPRoute Native ExternalAuth Construction Failures Must Seal Fail-Closed — FIXED

## Conclusion

Fixed. `PluginRuntime::from_httproute_filters_for_rule` and
`PluginRuntime::from_httproute_filters_for_backend` now call
`seal_fail_closed()` after building the Gateway API filter chain. If
`ForwardAuth::new` cannot build an outbound client, the invalid `ExternalAuth`
filter is omitted and an existing `RejectFilter` is prepended to the request
stage, returning the generic 500 fail-closed response before the backend is
contacted.

## Core Rationale

`ForwardAuth::new` intentionally returns `Err` for invalid targets, TLS, or
request-spec compilation. The HTTPRoute-native filter path records
`ForwardAuth` in `invalid_plugins`, but it is independent of the
`EdgionPlugins` compiler. Only the `EdgionPlugins` path previously sealed that
list, so a malformed inline auth filter could disappear while the route kept
serving traffic.

The seal belongs in both HTTPRoute constructors rather than in one current
caller. This keeps the constructor contract fail-closed for rule and backend
scope and protects future callers from silently dropping the same invalid
plugin. Valid filter chains are unchanged because sealing is a no-op when the
invalid list is empty.

The regression test covers both constructors with an invalid default target and
asserts that `RejectInvalidConfig` is the first request-stage filter. The
controller admission and secret-resolution gaps for the native `ExternalAuth`
shape, including the separate Kubernetes CRD shape mismatch, are not part of
this runtime fix and require a separate scope decision.

## Fix Suggestions Not Accepted

- Calling `seal_fail_closed()` only from `compile_http_rule` is not the
  canonical fix; it leaves the constructor unsafe for any other caller.
- Adding a new fail-open/fail-close option is not accepted. Authentication
  plugins use fixed fail-close semantics.
- Expanding this finding into Controller validation, native Gateway API CRD
  changes, or HTTPRoute secret resolution is not a narrow runtime fix. Track
  those as separate findings or tasks.

## Re-evaluation Triggers

- A new constructor records an invalid plugin but does not seal before returning
  a published runtime.
- `RejectFilter` no longer precedes request-stage filters or no longer returns
  the generic fail-closed response.
- Native `ExternalAuth` gains a distinct runtime construction path that bypasses
  both HTTPRoute constructors.

## Reference Cases

- `edgion-gateway/src/plugins/runtime/plugin_runtime.rs`
- `edgion-gateway/src/plugins/http/plugin_store.rs`
- `edgion-gateway/src/plugins/http/forward_auth/plugin.rs`
- `edgion-gateway/src/plugins/runtime/reject_filter.rs`
- Regression: `invalid_external_auth_seals_rule_and_backend_runtimes`
