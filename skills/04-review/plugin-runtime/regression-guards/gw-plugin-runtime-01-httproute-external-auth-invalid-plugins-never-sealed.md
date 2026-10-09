---
name: httproute-external-auth-runtime-fail-closed
description: Use when reviewing internal HTTPRoute Edgion-auth construction failures and invalid plugin sealing.
status: fixed
finding_id: gw-plugin-runtime-01
closed: 2026-08-02
knowledge_type: regression-guards
---

# Internal HTTPRoute Edgion-auth Construction Failures Must Seal Fail-Closed

## Conclusion

Fixed. `PluginRuntime::from_httproute_filters_for_rule` and
`PluginRuntime::from_httproute_filters_for_backend` now call
`seal_fail_closed()` after building the Gateway API filter chain. If
`ForwardAuth::from_route` cannot build an outbound client, the invalid `ExternalAuth`
filter is omitted and an existing `RejectFilter` is prepended to the request
stage, returning the generic 500 fail-closed response before the backend is
contacted.

## Core Rationale

The historical detached `ForwardAuth::new` constructor returned `Err` for invalid targets, TLS, or
request-spec compilation. The internal HTTPRoute filter path records
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
for internal rule and backend filters. Delegation checks parent and child source
rules through that same entry point. Invalid rules are excluded, valid siblings
survive, and correcting `clientHeaders` clears the invalid-rule status. If no
effective rule survives, the existing attachment rejection path applies.

This closes the static configuration validation gap for supplied ExternalAuth
payloads, including protected response-header names. Gateway construction and
sealing remain the defense for direct or stale inputs and runtime-only errors.
The original fix did not cover missing/mismatched payloads or material resolution.
The current ownership correction below addresses those internal paths. Native
Gateway API ExternalAuth remains unsupported.
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
  changes remained outside that correction; the later ownership work below covers
internal HTTPRoute material resolution.

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
- `CAND-admission-001` — internal Edgion-auth static admission parity follow-up.

## Current actual-field ownership boundary

The route constructor now requires a private `PreparedRouteAuth` made from the
actual effective route/filter loan. Source/Service identity, Controller failure
state and exact ordered TLS material proof precede client construction. The
canonical TLS pair projection exposes target identity only, allowing delegated
locality validation without inventing child ReferenceGrant authority. Original
authored tokens and pipeline authorization retain their full source checks.

Controller acquisition uses each original declaration before flattening, including
inherited intermediate and backend filters. Root-only receipts register valid
missing/denied material recovery before readonly bound acquisition with the
actual declaring owner. Root/raw-child transport inputs are scrubbed; parse/delete
replace dependencies and material; readonly status adds no edges. The default
delegation lookup cannot silently retain raw auth when resolution is unavailable.

New deterministic tests exercise actual event queues for Secret repair and Grant
revocation, declaration/locality inheritance, malformed/forged fields, exact TLS
identity failure, and both Gateway sealing paths. The prior test counts above are
historical evidence, not a fresh result for this change. Focused verification and
whole-repository gates are recorded by the active resource-reference task. Neither
these tests nor the internal decoder constitute live Kubernetes conformance.
