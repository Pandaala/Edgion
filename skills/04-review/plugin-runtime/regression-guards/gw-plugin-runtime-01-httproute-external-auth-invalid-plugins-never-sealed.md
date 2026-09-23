---
name: httproute-external-auth-runtime-fail-closed
description: Use when reviewing HTTPRoute-native ExternalAuth construction failures and invalid plugin sealing.
status: fixed
finding_id: gw-plugin-runtime-01
closed: 2026-08-02
knowledge_type: regression-guards
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

The original regression test covers both constructors with an invalid default
target and asserts that `RejectInvalidConfig` is the first request-stage filter.
That runtime fix did not cover Controller admission. The response-header policy
follow-up (`CAND-admission-001`) now calls the same
`ForwardAuthConfig::validation_error()` from the Controller's per-rule checks
for native rule and backend filters. Delegation checks parent and child source
rules through that same entry point. Invalid rules are excluded, valid siblings
survive, and correcting `clientHeaders` clears the invalid-rule status. If no
effective rule survives, the existing attachment rejection path applies.

This closes the static configuration validation gap for supplied ExternalAuth
payloads, including protected response-header names. Gateway construction and
sealing remain the defense for direct or stale inputs and runtime-only errors.
Missing/mismatched ExternalAuth payload handling, secret resolution, and the
Kubernetes CRD shape mismatch are separate existing limitations, not fixed here.
The concrete `target`/`decision.clientHeaders` trigger is reachable through the
shared FileSystem/etcd decoder; it is not a Kubernetes CRD compatibility claim.

Targeted regressions cover FileSystem/etcd decoding, rule/backend status,
valid siblings, all-invalid rejection, recovery to ordinary headers, delegated
parent/child rules, and both Gateway constructors. See test names containing
`external_auth_response_header_policy` in the HTTPRoute handler, delegation
handler, and plugin runtime. These are deterministic tests, not live deployment
or Kubernetes schema conformance evidence.

Follow-up verification: the two Controller tests and the Gateway constructor
test selected by `external_auth_response_header_policy` passed. The freshly
built Controller test binary also passed all 48 HTTPRoute handler tests and
all 40 delegation tests. Independent read-only review returned LGTM.

## Fix Suggestions Not Accepted

- Calling `seal_fail_closed()` only from `compile_http_rule` is not the
  canonical fix; it leaves the constructor unsafe for any other caller.
- Adding a new fail-open/fail-close option is not accepted. Authentication
  plugins use fixed fail-close semantics.
- The original runtime fix excluded Controller admission. The later response
  policy task explicitly required Controller/Gateway validation parity and
  authorized the static validation follow-up above. Native Gateway API CRD
  changes and HTTPRoute secret resolution remain outside that correction.

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

## Resolution references

- `gw-plugin-runtime-01` — 2026-08-02.
- `CAND-admission-001` — native ExternalAuth static admission parity follow-up.
