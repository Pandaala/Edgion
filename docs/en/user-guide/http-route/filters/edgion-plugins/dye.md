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
      - name: X-Release-Track          # on omitted => [success]; value omitted => plugin-recorded value
    response:
      - name: X-Edgion-Outcome
        on: [fallback, reject]
      - name: X-Edgion-Degraded
        on: [fail-open, fail-close]
        value: degraded                # operator-authored literal override
        conditions:                    # optional; reuses the same PluginConditions shape as entry-level `conditions`
          run:
            - type: keyExist
              key: { type: header, name: X-Debug-Token }
      - name: X-Edgion-Debug
        on: [fallback]
        value: "fallback for ${header:X-Account-Id}"   # template on the shared ${type:name} engine
  config:
    # ... plugin config; dye is NOT nested under config ...
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

## Capability table — which plugin types record which outcomes

Configuring an outcome a plugin type never records is an admission error
(rejected at the Controller and at Gateway compile), not a header that
silently never appears:

| Plugin | Outcomes it can dye |
|---|---|
| `Canary`, `DynamicInternalUpstream`, `DynamicExternalUpstream`, `DirectEndpoint` | `success`, `fallback`, `reject` |
| `RegionRoute` | `success`, `fallback` (no `reject` — RegionRoute has no `onMissing`/`onNoMatch`/`onInvalid` reject-shaped config path in v1) |
| `RateLimitRedis`, `ExtProc`, `Wasm`, `Dsl`, `OpenidConnect` | `fail-open`, `fail-close` |
| `ForwardAuth`, `LdapAuth` | `fail-close` only (auth class never fails open by policy) |
| Everything else | none |

The plugin-type → capability mapping is `EdgionPlugin::dye_capabilities`
(`edgion-resources/src/resources/edgion_plugins/edgion_plugin.rs`) — the
table above must match it; treat that function as the source of truth if
they ever diverge.

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

Before a plugin runs — before its `enable: false` kill switch, any protocol
gating, and its own entry-level `conditions` — the runtime removes the
entry's configured **request**-direction rule names from the incoming
request. This targets only the client-supplied value; it is a stripped-first,
render-after-if-triggered sequence, not a toggle:

- the strip runs **once per header name per request**, at the first entry in
  the pipeline that declares that name — a value rendered by an earlier
  entry's own request-direction dye rule is gateway-authored and is never
  re-stripped by a later entry (or a later runtime tier: route → backend →
  gateway → global run in sequence) that happens to share the same header
  name;
- the strip runs even when the plugin is kill-switched (`enable: false`) or
  when its entry-level `conditions` skip it — conditions gate whether a rule
  *renders*, never whether the strip runs. A disabled or condition-skipped
  plugin's configured dye names are still removed from client input.

This means a client can never forge a value under a name your route has
configured for dye, regardless of which path the matching plugin takes on a
given request.

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

Each rule may carry an optional `conditions` block, reusing the same
`PluginConditions` run/skip shape used by entry-level `conditions`. See
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
- Design record: [`2026-07-20-entry-level-dye-suite-design.md`](../../../../../../docs/superpowers/specs/2026-07-20-entry-level-dye-suite-design.md)
  (full mechanism, validation, and runtime rendering detail).
