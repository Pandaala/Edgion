---
name: resource-plugin-metadata
description: PluginMetaData resource — plugin system (similar to EdgionPlugins), supporting Global RegionConf + Service-level state management.
---

# PluginMetaData resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

PluginMetaData is Edgion's custom-extension resource that carries one `MetaDataEntry` variant (a tagged union with discriminator `spec.metadata.type`). There are currently **5 variants**:

1. **Data lists**: `KeyList` (which unifies the legacy StringList + RegexList, distinguished by `matchMode: Exact|Regex`) and `IpList` (CIDR list)
2. **RegionRoute system**: `ClusterRegionRoute` (full topology + routing rules) and `ServiceRegionRoute` (service-level failover, referencing a ClusterRegionRoute PM via `clusterRef`)
3. **L4 connection restriction**: `GlobalConnectionIpRestriction` (referenced by EdgionStreamPlugins via `pluginMetadataRef`)

## Source code locations

- Type definitions: `edgion-resources/src/resources/plugin_metadata/mod.rs` (main module), `edgion-resources/src/resources/plugin_metadata/region_route.rs` (RegionRoute config structs)
- Plugin types: `MetaDataPlugin` enum (ClusterRegionRoute / ServiceRegionRoute variants)
- Controller Handler: `handlers/plugin_metadata.rs`
- EdgionPlugins Handler (validation logic): `handlers/edgion_plugins.rs`
- PluginMetaDataStore (Controller side): `plugin_metadata_utils/plugin_metadata_store.rs`
- PluginMetaDataGatewayStore (Gateway side): `src/core/gateway/plugins/runtime/plugin_metadata/conf_handler.rs`
- RegionRoute tables (Controller): `region_route_table/cluster_table.rs`, `region_route_table/service_table.rs`
- Controller API: `src/core/controller/api/region_route_handlers.rs`

> The federation Center side (Center API list/failover/consistency handlers, CenterMetaDataStore, Center Watch cache, and the shared metadata ConfHandler) has been extracted to the separate EdgionCenter repository.

## Plugin system

`spec.plugins` is an array of tagged unions, formatted:

```yaml
spec:
  plugins:
    - type: ClusterRegionRoute    # cluster topology + routing rules (no baseInfo wrapper)
      config:
        myRegion: ...
        regions: [...]
        keyGet: [...]
        routeRules: [...]
    - type: ServiceRegionRoute   # service-level failover + clusterRef reference
      config:
        clusterRef:
          name: global-region-conf
          namespace: edgion-system
        regions: [...]
```

### MetaDataEntry enum (5 variants)

| Variant | Purpose | External reference |
|------|------|---------|
| `KeyList` | Key-value list, supports `Exact` / `Regex` match modes (consumed indirectly by KeyAuth, IpRestriction, etc.) | none |
| `IpList` | CIDR list (IP allowlist/denylist plugins) | none |
| `ClusterRegionRoute` | Cluster topology + routing rules + matchMap (operator/Center managed) | none |
| `ServiceRegionRoute` | Service-level failover + `clusterRef` referencing a ClusterRegionRoute PM | `clusterRef` → ClusterRegionRoute PM |
| `GlobalConnectionIpRestriction` | L4 connection-stage IP restriction with profile switching capability (Center-managed, consumed by EdgionStreamPlugins) | none |

## RegionRoute two-layer architecture

```
ClusterRegionRoute PluginMetaData ←── ServiceRegionRoute PluginMetaData
│ myRegion                           │ clusterRef → ClusterRegionRoute PluginMetaData
│ regions (full topology)            │ regions (per-region failoverTo override)
│ keyGet                             │
│ hashCalc                           │
│ routeRules                         │
│ routeByKeyConfMatch (matchMap)     │
│ (no baseInfo wrapper, no autoFailover) │
└─────────────────────────────────── EdgionPlugins.serviceRegionRouteRef ──┘
                                     (RegionRoute plugin, only one ref)
```

### Resolution chain (real-time lookup at Gateway request time)

```
EP.serviceRegionRouteRef
  → ServiceRegionRoute PluginMetaData (looked up by name in EP's namespace)
    → ServiceRegionRoute PluginMetaData.clusterRef
      → ClusterRegionRoute PluginMetaData (namespace/name)
```

At request time, the Gateway resolves this chain in real time from the global `PluginMetaDataGatewayStore` (ArcSwap + pre-indexed typed maps), with lock-free, zero-copy reads. The configuration is **not** pre-replicated by the Controller into EP — the Gateway looks it up on demand.

### ClusterRegionRoute PluginMetaData (cluster configuration)

Pre-built by Center / operator and shared by all RegionRoutes. Has **no `baseInfo` wrapper** — all fields sit directly under `config` — and contains **no `autoFailover`** (removed).

```yaml
apiVersion: edgion.io/v1
kind: PluginMetaData
metadata:
  name: global-region-conf
  namespace: edgion-system
spec:
  plugins:
    - type: ClusterRegionRoute
      config:
        myRegion: "virginia"
        regions:
          - name: virginia
            hashRange: [0, 599]
            backendEndpoint: "virginia.example.com"
          - name: ohio
            hashRange: [600, 999]
            backendEndpoint: "ohio.example.com"
        keyGet:
          - type: header
            name: X-Account-Id
        hashCalc:
          algorithm: crc32
          modulo: 1000
        routeRules:
          - type: RouteByKeyConfMatch
          - type: RouteByHashRange
        routeByKeyConfMatch:
          matchMap:
            "global-key": virginia
```

### ServiceRegionRoute PluginMetaData (business configuration)

**Must be created in advance by an administrator**; the Controller does not create it automatically. Contains a `clusterRef` pointing to the corresponding ClusterRegionRoute PluginMetaData.

```yaml
apiVersion: edgion.io/v1
kind: PluginMetaData
metadata:
  name: my-svc-pm
  namespace: default
spec:
  plugins:
    - type: ServiceRegionRoute
      config:
        clusterRef:
          name: global-region-conf
          namespace: edgion-system
        regions:
          - name: virginia
            failoverTo: ohio
```

### EdgionPlugins reference

In EdgionPlugins, `RegionRouteConfig` has only the `serviceRegionRouteRef` field with no inline routing configuration:

```yaml
- type: RegionRoute
  config:
    serviceRegionRouteRef: my-svc-pm
```

`serviceRegionRouteRef` is the name of a ServiceRegionRoute PluginMetaData in EP's own namespace (a plain string with no namespace field).

### Gateway real-time resolution (PluginMetaDataGatewayStore)

The Gateway does not cache resolution results; it walks the entire reference chain in real time on every request via `PluginMetaDataGatewayStore`:

- **Storage**: ArcSwap holding pre-indexed typed maps (`ClusterRegionRouteMap`, `ServiceRegionRouteMap`); the key is `"namespace/name"`
- **Lookup**: `get_cluster_region_route(ns, name)` / `get_service_region_route(ns, name)` — ArcSwap load + HashMap get, zero-copy
- **Update**: when ConfHandler receives an increment, it rebuilds the typed maps and `ArcSwap.store`s them; the read path is lock-free

### Merge priority (Gateway final configuration)

| Priority | Source | Notes |
|--------|------|------|
| 1 | All ClusterRegionRoute PluginMetaData configuration | `regions`, `keyGet`, `hashCalc`, `routeRules`, `matchMap` |
| 2 | ServiceRegionRoute PluginMetaData `regions[].failoverTo` | Sets `region.failover_to` (service-level override) |

> `autoFailover` has been entirely removed from the system: DNS health probes, RegionHealthTable, probe_loop, etc., are all gone. Only static `failoverTo` redirection remains.

### Center operations

> Center receives PM data from each Controller via reverse list/watch (federated gRPC streams) and stores it in `CenterMetaDataStore` (in-memory strongly typed, aggregated by Controller). Center **no longer** actively polls Controller HTTP endpoints to fetch PM data.

| Operation | Target | Endpoint |
|------|------|------|
| List cluster RegionRoutes | Aggregated list of ClusterRegionRoute PMs | `GET /api/v1/center/cluster-region-routes` |
| List service RegionRoutes | Aggregated list of ServiceRegionRoute PMs | `GET /api/v1/center/service-region-routes` |
| Cluster region failover (static) | `regions[].failoverTo` of ClusterRegionRoute PluginMetaData | `POST /api/v1/center/cluster-region-routes/failover` |
| Service-level failover | `regions[].failoverTo` of ServiceRegionRoute PluginMetaData | `POST /api/v1/center/service-region-routes/failover` |
| Cluster consistency check | Compare data consistency across Controllers | `GET /api/v1/center/cluster-region-routes/consistency` |
| Service consistency check | Compare data consistency across Controllers | `GET /api/v1/center/service-region-routes/consistency` |
| Cluster matchMap | `routeByKeyConfMatch.matchMap` of ClusterRegionRoute PluginMetaData | PUT PluginMetaData via proxy |

### Controller API

| Endpoint | Description |
|------|------|
| `GET /api/v1/cluster-region-routes` | List all ClusterRegionRoute PluginMetaData |
| `GET /api/v1/service-region-routes` | List all ServiceRegionRoute PluginMetaData |
| `POST /api/v1/cluster-region-routes/failover` | Modify failoverTo for a region in a Cluster PM |
| `POST /api/v1/service-region-routes/failover` | Modify failoverTo for a region in a Service PM |

> **API contract**: the body of the failover endpoints above (both Center and Controller) must be exactly
> `{namespace, name, regionName, failoverTo}` — four fields. Any extra fields (e.g., `myRegion`,
> `spec`, full PluginMetaData YAML) are rejected with 4xx during deserialization. This contract is
> enforced independently on both ends by the strongly-typed `FailoverRequest` + `deny_unknown_fields`:
> Center re-serializes before fanning out to Controllers and never forwards upstream raw bytes verbatim.

## Controller-side processing

### preparse

- Calls `PluginMetaData::validate_pre_parse()` to collect per-variant validation errors (invalid CIDR, invalid regex, GCIR `validate_and_init()` failures, etc.) and returns `Vec<String>`, used as the basis for `Accepted=False/Invalid`

### parse

- Update `PluginMetaDataStore` (used by EdgionPlugins and EdgionStreamPlugins handlers when resolving cross-PM references)
- Update `RegionRouteClusterTable` (`ClusterRegionRoute` variant) or `RegionRouteServiceTable` (`ServiceRegionRoute` variant)
- **Cross-PM ref registration**: for the `ServiceRegionRoute` variant, register `cluster_ref` in `plugin_metadata_refs` with key format `"<cluster_ns>/<cluster_name>"` and value `"PluginMetaData:<this_ns>/<this_name>"`. First `clear_refs_from` clears this resource's existing refs and then re-registers, preventing stale refs after a spec update

### on_change

- Look up resources referencing this PluginMetaData via `plugin_metadata_refs` (`PluginMetaDataRefManager`) and requeue them. Consumers include:
  - EdgionPlugins (via the chain `serviceRegionRouteRef` → ServiceRegionRoute PM → `clusterRef` → ClusterRegionRoute PM)
  - EdgionStreamPlugins (via `GlobalConnectionIpRestriction.pluginMetadataRef` → GCIR PM)
  - **Other PluginMetaData** (ServiceRegionRoute → ClusterRegionRoute, the new PM→PM reverse requeue)
- Requeue dispatches to the corresponding processor by splitting on `:` per the `<Kind>:<ns>/<name>` convention via `ep_key.split_once(':')`

### on_delete

- Remove from `PluginMetaDataStore`
- Remove from `RegionRouteClusterTable` / `RegionRouteServiceTable`
- `clear_refs_from` clears the cross-PM outbound refs registered by this resource
- Requeue all resources depending on this PM (so their `ResolvedRefs` is recomputed as False)

### update_status

PluginMetaData status follows Gateway API conventions and writes two conditions (using `k8s_openapi::Condition`):

| Condition | Semantics | Driver |
|-----------|------|------|
| `Accepted` | Whether the spec passes preparse validation | The validation_errors returned by preparse; empty → `True`/`Accepted`, non-empty → `False`/`Invalid` |
| `ResolvedRefs` | Whether external references resolve | Only meaningful for `ServiceRegionRoute.clusterRef`; other variants are always `True`. Not found, or target is not ClusterRegionRoute → `False`/`InvalidPluginMetaDataRef` (`ResolvedRefsError::PluginMetaDataNotFound` variant) |

`last_transition_time` is refreshed only when the status value changes.

## EdgionPlugins-side validation logic

The Controller does **not** copy routing configuration into EdgionPlugins, nor does it auto-create ServiceRegionRoute PluginMetaData. The EdgionPlugins handler performs a **whole-chain** validation (`serviceRegionRouteRef` → ServiceRegionRoute PM → `clusterRef` → ClusterRegionRoute PM), and the result is written to `EdgionPlugins.status.conditions[Accepted]`:

1. Check whether `serviceRegionRouteRef` is set (record a validation error if not)
2. Look up the ServiceRegionRoute PluginMetaData in `PluginMetaDataStore` (record if not found)
3. Check whether the located PM contains the `ServiceRegionRoute` variant and whether its `clusterRef` resolves to a ClusterRegionRoute PM

In parallel, PluginMetaData's own status independently reflects the resolution of its **direct** reference (`ServiceRegionRoute.clusterRef` → ClusterRegionRoute PM, see the update_status table above). This is a **healthy two-layer redundancy**:
- The PM-layer `ResolvedRefs` answers "can this ServiceRegionRoute PM's own direct reference resolve?"
- The EP-layer `Accepted` answers "can the entire EP → PM → PM chain be wired up?"

**ServiceRegionRoute PluginMetaData must be created in advance by an administrator**; the Controller does not create it automatically.

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| ServiceRegionRoute PluginMetaData → ClusterRegionRoute PluginMetaData | PluginMetaData | `clusterRef` field | Service PM links to a Cluster PM; the PM's own `ResolvedRefs` condition reflects the resolution result |
| PluginMetaData ← EdgionPlugins | EdgionPlugins | `serviceRegionRouteRef` | EP references a ServiceRegionRoute PM |
| PluginMetaData ← EdgionStreamPlugins | EdgionStreamPlugins | `GlobalConnectionIpRestriction.pluginMetadataRef` | ESP references a GCIR PM; ESP's own `ResolvedRefs` reflects the resolution result |
| PluginMetaData → {EdgionPlugins, EdgionStreamPlugins, PluginMetaData} | various | `plugin_metadata_refs` | PM changes trigger requeue of consumers; ClusterRegionRoute PM changes requeue the ServiceRegionRoute PMs depending on it (the new PM→PM reverse requeue) |
