---
name: entry-level-dye
description: Canonical entry-level dye capability, validation, producer ownership, and response-header timing contract.
---

# Entry-level dye

## Ownership and validation

`EdgionPlugins` entries own `DyeSuite { request, response }`. Each `DyeRule`
selects outcomes through `on`, a header through `name`, an optional shared
template through `value`, and an observational render gate through `conditions`.
The direction is a header destination, not an execution stage.

`EdgionPlugin::dye_capabilities(DyeStage)` in `edgion-resources` is the single
capability implementation. Entry validators pass their configured stage; Controller
`validate_plugins` and Gateway runtime construction call the same validators.
The result describes supported producer paths under static configuration, not a
promise that every request, arbitrary script, condition, or routing table matches.
Do not infer capabilities from the currently available backend/overlay generation.

Unsupported outcomes report a configuration error and prevent that entry's dye
plan from being compiled. They preserve the existing non-fatal dye-error policy:
the plugin itself can remain active. Controller `Accepted=False` is advisory;
Gateway independently excludes the invalid dye plan. A valid plan replaces the
previous generation on updates; an invalid dye update must not retain stale rules.
Template/compiler disagreement remains an internal fail-closed compilation error.

## Stage and producer inventory

Paths below are relative to `edgion-gateway/src/plugins/http/`. Outcomes are the
union of intentional producers in the listed paths after applying configuration.

| Plugin | Configured stage / producer paths | Effective outcomes |
|---|---|---|
| Canary | Request; `canary/plugin.rs::run_request` | Success plus `onInvalid` outcome. Internal `config.enable=false` pauses execution but retains the anti-spoof plan. |
| DynamicInternalUpstream | Request; `dynamic_internal_upstream/plugin.rs::run_request` | Success, `onMissing`, `onInvalid`; `onNoMatch` only in rules mode. |
| DynamicExternalUpstream | Request; `dynamic_external_upstream/plugin.rs::run_request` | Success with a nonempty domain map, `onMissing`, `onNoMatch`. |
| DirectEndpoint | Request; `direct_endpoint/plugin.rs::run_request` | Success, `onMissing`, `onInvalid`. |
| RegionRoute | Request; `region_route/plugin.rs::compute_effective_regions`, `route_to` | Success needs routing rules and a region source; Fallback needs an overlay reference and a non-inert base configuration. Never Reject. |
| RateLimitRedis | Request; `rate_limit_redis/plugin.rs::apply_redis_failure_policy` | `onRedisFailure: Allow` selects FailOpen; Deny selects FailClose. A normal quota rejection is not a dependency failure. |
| ExtProc | Request entry owns request headers/body and attached response-header processing; `ext_proc/plugin.rs::fail_mode` | `failureModeAllow` selects FailOpen or FailClose. Request-stream failures use the entry plan before response-header commit. |
| Wasm | Request or upstream response filter; `wasm/plugin.rs::fail_mode` | `failOpen` selects FailOpen or FailClose. Response-body observation produces no dye. |
| DSL | Request or upstream response filter; `dsl/lang/vm.rs::execute_safe`, `apply_error_policy` | Deny/DenyWith select FailClose. Ignore permits FailOpen plus fixed FailClose for BodyAccess errors, including string-regex scanner errors and cached request-body reads. No arbitrary-script reachability solver. |
| OpenidConnect | Request; `openid_connect/discovery_jwks.rs::get_or_fetch_jwks` | JWKS stale-cache FailOpen and unavailable FailClose. JWKS is reachable with JwksOnly, Auto+useJwks, useNonce (callback validation), or session renewal (`bearerOnly: false` and `renewAccessTokenOnExpiry: true`). The session refresh timeout path invokes JWT verification independently of the bearer verification mode, before checking stale-token leeway. Introspection verification itself emits neither. |
| ForwardAuth / LdapAuth | Request; their dependency-failure paths in `plugin.rs` | FailClose only; ordinary credential denial is not dependency failure dye. |
| AiProxy and other variants | None | No public dye capability or entry-level producer. Native WAF outcome diagnostics use its own existing transaction/log mechanism, not `PluginSession::record_outcome`. |

The internal `DyeStage` is not serialized and adds no CRD field. There is no
directly configurable async upstream-response dye producer today. No response-body
stage accepts header dye, even though Wasm itself supports body observation.

## Rendering lifetime and immutable plans

`CompiledDyePlan` compiles templates and conditions once; the runtime installs the
entry plan before executing the plugin. `PingoraSessionAdapter::record_outcome`
is the single renderer. Request Success writes the upstream request header.
Response Success uses Set; other outcomes use Append. Before response headers
exist these operations are queued through the canonical response-header operation
queue. Response-header callbacks use a bound mutable header directly. Existing
local-reply handling reapplies queued operations before sending the replacement.

Once response headers commit, body/trailer callbacks cannot change those headers.
ExtProc response-body and trailer processors therefore neither record header dye
nor retain a dye plan for that purpose. Their existing terminal decisions,
diagnostics, trailer mutations, and streaming/backpressure behavior remain intact.
A stream termination after headers commit is not a replacement HTTP error response.
Do not add response aggregation, per-chunk logs, or a second header mutation path
to make late dye appear. Adding a future producer requires documenting its commit
boundary and extending the parity tests before advertising the capability.

## Inert entries and security boundaries

- Entry `enable=false` and policy Bypass/Block skip normal dye validation and
  compilation. A policy Block retains its existing denial behavior.
- An active entry's internal Canary kill switch and runtime conditions do not
  disable anti-spoof stripping: strip each declared request header once before
  execution. A name written by an earlier dye entry is not stripped again.
- Dye conditions with unresolved references remain observational non-matches;
  malformed conditions remain configuration errors. Entry/body conditions keep
  their distinct fail-closed reference policy.
- Shared header validation/sanitization, request-key collision checks, success
  versus failure separation, and cross-entry Set/Append collision checks remain.
  `secretRef` is forbidden in client-visible dye values. Logging remains owned
  by the canonical access-log/PluginLog mechanisms; no new hot-path tracing.

See the [user reference](../../../docs/en/user-guide/http-route/filters/edgion-plugins/dye.md)
and the [regression guard](../../04-review/plugin-runtime/regression-guards/dye-stage-producer-parity.md).
