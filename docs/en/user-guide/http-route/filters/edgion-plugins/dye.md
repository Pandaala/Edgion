# Dye (Traffic-Tagging Headers)

`dye` is an entry-level mechanism for tagging a request or response with
operator-chosen HTTP headers based on what a plugin actually did — which
target it selected, or which failure behavior fired. It is not a plugin of
its own; it is a sibling field of `type`/`config`/`alias`/`conditions` on any
plugin entry that supports it.

This page is the single canonical reference for `dye`. The per-plugin pages
([Canary](./canary.md), [Dynamic Upstream](./dynamic-upstream.md),
[Direct Endpoint](./direct-endpoint.md), [RegionRoute](./region-route.md), and
the dependency-failure plugins) link back here for the shared vocabulary and
show only what is specific to that plugin (its value provenance, protocol
gating, and which outcomes it actually records).

## Shape

```yaml
- type: Canary
  enable: true
  dye:
    request:
      - name: X-Release-Track
    response:
      - name: X-Edgion-Outcome
        on: [reject]
        value: "selection rejected for ${header:X-Account-Id}"
        conditions:
          run:
            allOf:
              - type: keyExist
                key: { type: header, name: X-Debug-Token }
  config:
    enable: true
    activeProfile: stable
    profiles:
      stable: { name: app-v1 }
    onInvalid: reject
```

`DyeSuite { request: [DyeRule], response: [DyeRule] }`;
`DyeRule { name, on: [DyeOutcome] (default [success]), value?, conditions? }`.

## Outcome vocabulary (`on`)

| Outcome | Meaning | Write semantics |
|---|---|---|
| `success` | The plugin resolved and used a target/selection. | Set (replace-all) |
| `fallback` | A selection plugin degraded to normal routing instead of its own selection. | Append (no dedup) |
| `reject` | A selection plugin rejected the request instead of selecting. | Append (no dedup) |
| `fail-open` | A dependency-failure plugin continued the request despite its dependency failing. | Append (no dedup) |
| `fail-close` | A dependency-failure plugin rejected the request because its dependency failed. | Append (no dedup) |

A rule's `on` list may contain any combination of these, **except** it must
not mix `success` with any failure outcome (`fallback`/`reject`/`fail-open`/
`fail-close`) in the same rule — the two families have different value
provenance and different write semantics (Set vs Append), so one rule cannot
serve both. Use two rules with the same header `name` if you want both a
success value and a failure value on the same header — the failure Append
will not clobber a prior success Set as long as the two rules do not use the
same name within one resource (see the Set-clobber validation below).

**Directions**: `request`-direction rules are valid only with `on: [success]`
in v1 — request-direction failure dye (surfacing degraded mode to the
upstream) is a documented future extension, not supported today. Put failure
outcomes only on `response`-direction rules.

## Capability by stage and configuration

Controller and Gateway share `EdgionPlugin::dye_capabilities(DyeStage)`.
An unsupported outcome reports a configuration error and disables that entry's
**dye plan**; it does not by itself disable the plugin or reject traffic.

| Plugin | Supported stage and outcomes |
|---|---|
| `Canary` | Request: `success` plus the configured `onInvalid` outcome. |
| `DynamicInternalUpstream` | Request: `success`, configured `onMissing`/`onInvalid` outcomes; `onNoMatch` only in rules mode. |
| `DynamicExternalUpstream`, `DirectEndpoint` | Request: selection success and the outcomes selected by applicable fallback/reject policies. |
| `RegionRoute` | Request: success with routing rules and a region source; fallback only with an applicable `overrideRef`; never reject. |
| `RateLimitRedis` | Request: `onRedisFailure: Allow` permits `fail-open`; `Deny` permits `fail-close`. |
| `ExtProc` | Request entry, including attached callbacks before response headers commit: the outcome selected by `failureModeAllow`. |
| `Wasm` | Request or upstream response filter: the outcome selected by `failOpen`. No response-body dye. |
| `Dsl` | Request or upstream response filter: `deny`/`denyWith` permit `fail-close`; `ignore` also permits `fail-open`. Hard body/scanner errors remain fail-close. |
| `OpenidConnect` | Request: JWKS stale-cache `fail-open` and unavailable `fail-close`, when JWKS verification, nonce validation, or session renewal (`bearerOnly: false`, `renewAccessTokenOnExpiry: true`) is enabled. Session refresh timeouts can reach JWKS even with `IntrospectionOnly`; introspection verification itself produces neither. |
| `ForwardAuth`, `LdapAuth` | Request: `fail-close` for dependency failures. |
| `AiProxy` and other plugins | No entry-level dye. |

Capabilities describe supported execution paths, not a guarantee that every
request or script emits an outcome. Conditions, protocol gating and Canary's
internal kill switch can suppress execution while preserving valid declarations.
Choose only outcomes supported by the effective policy; for example,
`Wasm failOpen: true` cannot subscribe to `fail-close`.

Response headers must still be mutable when an outcome is recorded. Request-stage
response dye is queued; response-header callbacks write the current header.
ExtProc response-body/trailer results and Wasm body observation occur too late
and do not produce response-header dye. A stream terminated after headers commit
cannot replace the already-sent status or add a dye header.

## Value provenance

`value` has two provenances, both under one rule shape:

- **Omitted** — the plugin's own recorded value:
  - on `success`, the resolved selection value (e.g. the active profile
    name, the resolved backendRef `[ns/]name`, the resolved domain
    `domain:port`, the resolved region name) — unchanged from the original
    per-plugin dye behavior;
  - on a failure outcome, `<plugin_type>:<behavior>:<cause>` — snake_case
    plugin type, the outcome name, and a plugin-authored diagnostic cause,
    e.g. `rate_limit_redis:fail-open:redis-unreachable`,
    `canary:reject:on-invalid`.
- **Present** — an operator-authored template string on the shared
  `${type:name}` engine (see
  [unification registry entry 11](../../../../../../skills/01-architecture/00-common/05-unification-points.md)),
  of which a plain literal (e.g. `value: degraded`) is the degenerate case.
  The allow-list is every variable type the engine supports **except
  `secretRef`** — a dye value renders into a client-visible header, so a
  secret variable there would exfiltrate the secret value by configuration.
  Client-controlled variables (`header`/`query`/`cookie`) are permitted:
  they merely reflect input the client already knows, not something the
  operator did not intend to expose. A referenced variable that is absent on
  a given request renders as an empty string (the template engine's normal
  `ctx_set` convention), so a templated value collapses to its literal
  skeleton rather than failing.

## Stability contract (failure values)

For the omitted-value failure case, `<plugin_type>:<behavior>` is a stable
contract: `plugin_type` is fixed snake_case, `behavior` is the closed outcome
enum above minus `success`. Downstream tooling may match on this prefix. The
trailing `cause` segment is **best-effort diagnostic** and may change between
Edgion versions without notice — do not parse it as an API. Success values
keep their pre-existing contract (the bare resolved value, no prefix).

If you need less exposure than the full `cause` gives you, use a static
`value` override (e.g. `value: degraded`) — it drops the behavior/cause
detail entirely and renders your literal instead.

## Anti-spoof strip

For an active entry with a valid dye plan, the runtime removes configured
**request**-direction header names before plugin execution, protocol gating and
entry conditions. It strips each name once per request, so an earlier entry's
gateway-authored dye is not removed again.

Canary's internal `config.enable: false` pauses selection but retains this strip.
The outer entry `enable: false` and policy Bypass/Block are different: they do not
compile a normal dye plan. Invalid dye configurations also install no plan. A
condition-skipped active entry still strips; rule conditions only gate rendering.

## Header name validation

Every configured `name` is checked at admission (Controller ingest and
Gateway compile, single implementation):

- must pass the standard HTTP header-token syntax check;
- must not be one of the framing-sensitive denied names: `Host`,
  `Content-Length`, `Transfer-Encoding`, `Connection`, `TE`, `Upgrade`,
  `Trailer`;
- (request direction only) must not equal, case-insensitively, one of the
  plugin's own header-typed key sources (e.g. a `DynamicInternalUpstream`
  dye name colliding with its own `from: {type: header, name: ...}`) — the
  anti-spoof strip would otherwise remove the plugin's own routing input
  before it gets a chance to read it;
- (response direction) a failure rule's `name` must not equal,
  case-insensitively, a success rule's `name` anywhere in the same
  `EdgionPlugins` resource — a success Set queued after a failure Append
  would silently wipe the appended failure values. This is checked within one
  resource only; if you compose plugins from multiple `EdgionPlugins`
  resources via several `ExtensionRef` entries, use distinct names for
  success and failure dye yourself, since cross-resource composition is not
  statically checkable.

## `conditions`

Each rule may carry an optional `conditions` block, reusing the same explicit
`PluginConditions` `run`/`skip` ConditionSets used by entry-level conditions. See
[Plugin Conditions](./conditions.md) for variants, `negate`, and `values`.
Unlike the strip (which is unconditional), `conditions` gate whether a
*matching* outcome actually renders its header — this doubles as the primary
security mitigation for internet-facing routes (see below).

## Security guidance

A dye value — especially a failure value — reveals internal state to every
client on the route:

- `rate_limit_redis:fail-open:redis-unreachable` tells a client, in real
  time, that rate limiting is currently unenforced, which a probing client
  could use to time abuse to the fail-open window;
- causes like `ldap-unreachable` reveal which internal dependency is down
  (reconnaissance value).

Treat `dye` — and failure-outcome dye in particular — as opt-in
instrumentation for internal, canary, or trusted-tier traffic, not a
default-on feature for internet-facing routes. For internet-facing routes,
either:

- strip the header at your outer tier (CDN/ingress) before it reaches the
  public client, or
- (preferred) gate the rule with `conditions`, e.g. a debug-token header
  check, so the outcome only renders for diagnostic requests you control.

If you want the observability without the exposure, use a static `value`
override — it drops the `cause` (and even the `behavior`) detail and renders
a fixed literal instead of the plugin-recorded diagnostic string.

## Related documents

- [Canary](./canary.md), [Dynamic Upstream](./dynamic-upstream.md),
  [Direct Endpoint](./direct-endpoint.md), [RegionRoute](./region-route.md) —
  per-plugin value provenance and examples.
- [Filters Overview](../overview.md)
- Design record: [`entry-level-dye.md`](../../../../../../skills/01-architecture/02-gateway/entry-level-dye.md)
  (full mechanism, validation, and runtime rendering detail).
