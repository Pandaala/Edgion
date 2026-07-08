# Canary Plugin

> **🔌 Edgion extension**
>
> Canary is a full-cutover traffic-switch plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

## Overview

`Canary` sends **100% of a route's traffic** to one named backend at a time — the "active profile" — instead of splitting it by weight. It is the full-cutover / blue-green counterpart to `backendRefs[].weight`'s percentage-based split: where weight gradually shifts a fraction of traffic, Canary flips *all* of it, atomically, from one release to another.

The active profile can come from two places:

- **Base config** (`activeProfile` in the `EdgionPlugins` resource) — git-owned, requires a redeploy to change.
- **Runtime override** (`activeProfileRef`, a `Selector` `EdgionConfigData`) — hot-swappable without touching the `EdgionPlugins` resource; updating the referenced `EdgionConfigData` flips the active profile on all gateways referencing it within the standard conf_sync hot-reload window.

Each named profile points to a `backendRef` that **must already be listed** in the route's `backendRefs` — Canary only *selects among* the route's existing backends, it cannot jump to an arbitrary Service.

## Quick Start

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: canary-switch
  namespace: default
spec:
  requestPlugins:
    - enable: true
      type: Canary
      config:
        enable: true
        activeProfile: stable
        profiles:
          stable:
            name: canary-app-v1
          canary:
            name: canary-app-v2
        onInvalid: reject
        debugHeader: true
```

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: canary-test
  namespace: default
spec:
  parentRefs:
    - name: edgion-plugins-gateway
      namespace: edgion-test
      sectionName: http
  hostnames:
    - "canary-test.example.com"
  rules:
    - backendRefs:
        - name: canary-app-v1
          port: 30001
        - name: canary-app-v2
          port: 30002
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: canary-switch
```

With no `activeProfileRef` configured, every request is pinned to the `stable` profile (`canary-app-v1`) — 100% of the route's traffic, regardless of the `backendRefs[].weight` values on the route (Canary overrides weighted selection whenever it is enabled and resolves a valid profile).

## Configuration Reference

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enable` | Boolean | **Required** | Kill switch. `false` => the plugin is a no-op; the route's own weighted `backendRefs` selection applies unmodified. |
| `activeProfile` | String | **Required** | Base/default profile key; must exist in `profiles`. Used whenever no `activeProfileRef` is configured, and as the fail-to-base target on any selector resolution error. |
| `profiles` | Map\<String, DynUpstreamTarget\> | **Required** | Named profiles: profile name -> target backendRef. Git-owned, always safe to fall back to. Must be non-empty. |
| `profiles[].name` | String | **Required** | Name of the target `backendRef` (must match a `backendRef` in the route). |
| `profiles[].namespace` | String | none | Optional namespace of the target `backendRef`. If unset, matches by name only; if set, both name and namespace must match. |
| `activeProfileRef` | EdgionConfigDataRef | none | Optional reference to a `Selector` `EdgionConfigData` supplying the active profile name at runtime. |
| `onInvalid` | Enum | `Reject` | Behavior when the resolved profile's `backendRef` is not present in the route's `backendRefs` (stale/misconfigured profile). Allowed values: `Reject` (403) / `Fallback` (falls through to the route's own weighted `backendRefs` selection). |
| `debugHeader` | Boolean | `false` | When `true`, sets the `X-Canary-Active` request header (sent to the upstream) to the resolved profile name. |

### EdgionConfigDataRef (for `activeProfileRef`)

| Parameter | Type | Description |
|-----------|------|-------------|
| `name` | String | Name of the `EdgionConfigData` resource (`type: Selector`). |
| `namespace` | String | Namespace. Defaults to the `EdgionPlugins` namespace. |

Admission-time validation (`Accepted=False` if violated): `profiles` must be non-empty, and `activeProfile` must name a key that exists in `profiles`.

## Usage Scenarios

### Base-Only: Static Full Cutover

Without `activeProfileRef`, Canary behaves as a fixed override of the route's weighted selection — every request goes to `activeProfile`. Changing which backend serves traffic requires editing and reapplying the `EdgionPlugins` resource (a redeploy):

```yaml
requestPlugins:
  - type: Canary
    config:
      enable: true
      activeProfile: stable
      profiles:
        stable:
          name: app-v1
        canary:
          name: app-v2
```

This is useful when the cutover decision is part of your GitOps/CI pipeline rather than an operational, runtime action.

### Hot-Swap: Runtime-Switchable Cutover

Add `activeProfileRef` to point at a `Selector` `EdgionConfigData`. Flipping the selector's `active` field switches 100% of traffic to the named profile without redeploying the `EdgionPlugins` resource:

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: canary-selector
  namespace: default
spec:
  data:
    type: Selector
    config:
      active: stable   # change to "canary" to flip 100% of traffic to the canary profile
```

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: canary-switch
  namespace: default
spec:
  requestPlugins:
    - enable: true
      type: Canary
      config:
        enable: true
        activeProfile: stable
        profiles:
          stable:
            name: canary-app-v1
          canary:
            name: canary-app-v2
        activeProfileRef:
          name: canary-selector
          namespace: default
        onInvalid: reject
        debugHeader: true
```

Updating `canary-selector.spec.data.config.active` from `stable` to `canary` hot-swaps every gateway referencing it — no `EdgionPlugins` edit, no route edit, no restart.

### `onInvalid` Choice: `reject` vs `fallback`

If the resolved profile's `backendRef` is missing from the route's `backendRefs` (e.g. the profile was configured with a name that no longer matches any `backendRef` in the route), `onInvalid` decides what happens next:

```yaml
requestPlugins:
  - type: Canary
    config:
      enable: true
      activeProfile: stable
      profiles:
        stable:
          name: app-v1
        canary:
          name: app-v2   # if app-v2 is later removed from the route's backendRefs...
      onInvalid: reject  # ...requests resolving to "canary" get 403
```

- `reject` (default): return `403 Forbidden` with body `"Target backend not allowed"`. Fails closed — appropriate when a stale profile pointing at the wrong backend is a security concern (e.g. it could otherwise send traffic to a decommissioned or unintended Service).
- `fallback`: fall through to the route's own weighted `backendRefs` selection, as if Canary were disabled for this request. Fails open — appropriate when you would rather serve traffic via normal load balancing than reject the request outright.

### Debug Header

Set `debugHeader: true` to have the gateway attach `X-Canary-Active: <profile-name>` to the request forwarded to the upstream, so the resolved profile is visible for tracing/debugging:

```bash
curl -H "Host: canary-test.example.com" http://<gateway>/  # upstream sees X-Canary-Active: stable
```

This header carries the **profile name only** (e.g. `stable`, `canary`) — it does not (yet) support attaching arbitrary business-defined values per profile. See the FAQ below.

## Behavior Details

- **Kill switch (`enable: false`)**: the plugin is a complete no-op; the route's own weighted `backendRefs` selection applies unmodified. Access log records `disabled:pass`.
- **Selector resolution, fail-to-base**: when `activeProfileRef` is set, the plugin resolves the active profile name from the referenced `Selector` `EdgionConfigData` on every request (for hot-reload). Each fail-to-base branch falls back to `activeProfile` and emits a **distinct** `selector miss: <reason>` log tag so operators debugging "did my flip take?" can grep the access log for the exact failure mode:

  | Reason                            | Access-log tag                       | Cause                                                                                          |
  |-----------------------------------|--------------------------------------|------------------------------------------------------------------------------------------------|
  | Referenced object doesn't exist   | `selector miss: absent`              | The `activeProfileRef` name/namespace resolves to nothing in the EdgionConfigData store.       |
  | Envelope kill switch is off       | `selector miss: disabled`            | The referenced `EdgionConfigData` has `spec.enable: false`.                                    |
  | Wrong resource type               | `selector miss: type_mismatch`       | The referenced `EdgionConfigData` is not of type `Selector` (e.g. `KeyList`, `IpList`).        |
  | Unknown profile name              | `selector miss: unknown_profile`     | The `Selector.active` value names a profile that does not exist in this plugin's `profiles`.   |

  Denied cross-namespace refs are handled at plugin construction time: an `activeProfileRef` marked `ref_denied` by the controller (no matching `ReferenceGrant`) is treated identically to "no selector configured" — the plugin never looks it up in the store and no `selector miss` tag is emitted. A denied ref always resolves to the base `activeProfile`, never to an attacker-influenced value.

  One case is a deliberate no-op rather than a failure: if the `Selector`'s `active` field is unset (`null`), the plugin also uses the base `activeProfile` — but this is not logged as a miss, since an absent `active` is a valid "use base" configuration rather than an error.

  > Per project convention, these signals live in the access log rather than as Prometheus counters — aggregate them at query time in Elasticsearch / ClickHouse / Loki when you want a "canary switch failure rate by reason" dashboard.
- **backendRef membership check**: once a profile name is resolved (base or selector), the plugin verifies the profile's target `backendRef` (`name` + optional `namespace`) actually appears in the route's `backendRefs`. This check runs on every request against the *current* route state, so removing a backend from the route invalidates any profile pointing at it — governed by `onInvalid`.
- **`onInvalid` behavior**: see the Usage Scenarios section above.
- **`debugHeader`**: sets `X-Canary-Active` to the resolved profile name; only applied on a successful upstream jump (not when the plugin is disabled, has no route, or rejects).
- **No route / no backendRefs**: if the request has no route unit or the route rule has no `backendRefs`, the plugin passes through (`GoodNext`) without acting — this should not occur in practice, since `Canary` is only meaningful attached to a route with backends.

## FAQ

### Q1: How is Canary different from `backendRefs[].weight`?

**A**: `weight` performs a **percentage split** across backends for every request (e.g. 90/10) and requires editing the route to change the ratio. `Canary` always sends **100%** of traffic to one profile and can be flipped at runtime via `activeProfileRef` without touching the route or redeploying. Use `weight` for gradual rollout percentages; use `Canary` for an instant, remotely-switchable full cutover (blue-green style). See [Backend Weight](../../backends/weight.md).

### Q2: How is Canary different from `DynamicInternalUpstream`?

**A**: `DynamicInternalUpstream` selects a target **per request**, driven by request metadata (a header/query/cookie value on that specific request) — different requests can be routed to different backends simultaneously (e.g. a canary cookie pins one user while everyone else gets normal weighted selection). `Canary` selects a target **globally for the whole route**, driven by config/selector state — every request gets the same profile until the selector changes. See [Dynamic Upstream](./dynamic-upstream.md).

### Q3: What happens if I remove `activeProfileRef` entirely?

**A**: The plugin always uses the base `activeProfile`. This is the same end state as the selector resolving to "use base" — there is no behavioral difference between "no `activeProfileRef` configured" and "`activeProfileRef` configured but currently failing/absent/denied."

### Q4: Can I attach per-profile business headers (e.g. `X-Release-Track: canary` on the response, or a custom value per profile) instead of just the fixed `X-Canary-Active` debug header?

**A**: Not yet. `debugHeader` only supports a fixed header name (`X-Canary-Active`) whose value is the resolved profile name — it is for debugging/tracing, not for signaling business logic to the backend or client. Per-profile, business-defined "dye" headers (arbitrary name/value pairs configured per profile) are tracked as a cross-plugin design item in [`tasks/todo/plugin-dye-header-unification.md`](../../../../../../tasks/todo/plugin-dye-header-unification.md) — it is not implemented for Canary or any other plugin today.

## Related Docs

- [Dynamic Upstream](./dynamic-upstream.md) — per-request upstream selection driven by request metadata
- [Global Access Control](./global-access-control.md) — the plugin Canary's Selector fail-to-base logic mirrors
- [Backend Weight](../../backends/weight.md) — percentage-based traffic splitting
- [Filters Overview](../overview.md)
