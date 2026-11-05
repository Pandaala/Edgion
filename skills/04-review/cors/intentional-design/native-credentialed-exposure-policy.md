---
name: native-credentialed-exposure-policy
description: Use when reviewing native CORS credentialed wildcard exposure, validation, or publication.
knowledge_type: intentional-design
---

# Native credentialed CORS requires explicit exposure names

## Conclusion and scope

The user approved rejecting native HTTPRoute CORS configurations where
`HTTPCORSFilter.allow_credentials == Some(true)` and `expose_headers` contains an
entry exactly equal to `*`, including mixed lists. This invalidates the whole rule;
valid siblings remain active. With no surviving effective rule, an authored policy
violation rejects the Gateway attachment with `Accepted=False/UnsupportedValue`.
A delegation child is validated as its own route; its diagnostic stays on the child.
False/omitted credentials preserve wildcard output. Explicit names are joined;
empty/omitted lists emit no exposure header. No browser-mode inference is added.

## Rationale and evidence

D0086 identified unchanged wildcard output despite credentials being permitted.
[Gateway API v1.5.0](https://github.com/kubernetes-sigs/gateway-api/blob/v1.5.0/apis/v1/httproute_types.go#L1458-L1496)
forbids wildcard exposure on credentialed responses but leaves the implementation
policy open. [Fetch revision 394d20d](https://github.com/whatwg/fetch/blob/394d20d144ed1401c2c0e02c35bc3608cb2a2269/fetch.bs#L5079-L5091) expands the
wildcard only when credentials mode is not `include`; otherwise it is a literal
header name. This alone does not fail the entire CORS request or expose all headers.
Cookie/Authorization presence does not reliably identify browser credentials mode.

The shared `HTTPCORSFilter::credentialed_expose_headers_error` feeds `validate`
and `HTTPRouteRule::unsupported_cors_reason`. Controller invalid-rule processing,
attachment status, and Gateway rule publication use these checks. `GapiCorsFilter::try_new` also validates. Constructor rejection
alone is insufficient: the runtime builder can omit a failed filter, so the Gateway
must reject the rule before compilation even without Controller invalid indices.

## Alternatives and limits

Rejection is an explicitly chosen support restriction, not a Gateway API admission
requirement. Operators must list exposed names. Omission would silently discard the
requested exposure; literal output would preserve the misleading configuration;
enumerating every upstream header would broaden exposure and add response-path work.
No origin, method, allowed-request-header, or EdgionPlugins CORS behavior changes.
The existing HTTPRouteCORS feature declaration does not prove support for the
rejected combination. Other native CORS validation gaps remain separately tracked.

## Verification and re-evaluation

Focused tests named `credentialed_expose_headers` cover shared validation, runtime
header values, status recovery, and publication without status or with resolved rules.
The new `test_native_cors_expose_headers.py` client covers shared-topology publication,
wire output and invalid/valid hot reload without changing existing integration files.
Verified on 2026-09-10: all five new focused tests and 72 CORS-filtered
unit tests passed. The shared-topology client passed all seven behavior phases
(including delegated rejection/recovery) and removed/unpublished all four temporary
routes. Fresh Controller/Gateway builds, workspace check, Clippy, formatting,
agent-doc consistency, SSA force, metrics inventory, Pingora source policy,
gateway tracing boundary, and English-only checks passed. Clippy reports existing
warnings. Independent review returned LGTM after correcting the delegated diagnostic
call site. Full workspace tests and Kubernetes conformance were not rerun.
Re-evaluate when Gateway API defines a required replacement
behavior, browser semantics change, or full wildcard expansion is requested.

## Resolution references

- D0086 (B0124), originally needs-discussion: real wildcard behavior gap; the user
  selected explicit-list enforcement rather than accepting the old behavior.
- Source task: `decide-credentialed-native-cors-expose-headers-wildcard.md`.
