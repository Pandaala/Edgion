# RegionRoute

RegionRoute selects a region using request keys and ordered routing rules. It is an
EdgionPlugins extension. A required `clusterRegionRef` references an EdgionConfigData
whose `spec.data.type` is `ClusterRegionSetting`.

## Resource ownership

The setting owns `clusterName`, `regionName` and the complete `regions` table.
`regionName` is the sole local-region identity; it must name a row in that table.
The plugin owns request extraction, hashing, route rules and failure policy.
`clusterName` and `regionName` are nonblank, case-sensitive identities of at most
63 UTF-8 bytes, with no control characters. They are not normalized.

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: local-region
  namespace: production
spec:
  enable: true
  data:
    type: ClusterRegionSetting
    config:
      clusterName: production
      regionName: us-east
      regions:
        - name: us-east
          hashRange: [0, 499]
          backendEndpoint: api.us-east.example.com:443
          tls: true
        - name: eu-west
          hashRange: [500, 999]
          backendEndpoint: api.eu-west.example.com:443
          tls: true
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: regional-routing
  namespace: production
spec:
  requestPlugins:
    - type: RegionRoute
      config:
        clusterRegionRef:
          name: local-region
        failOpen: true
        statusOnError: 503
        keyGet:
          - type: header
            name: X-Account-Id
        hashCalc:
          algorithm: crc32
          modulo: 1000
          writeBack:
            - type: responseHeader
              name: X-Region-Hash
        routeRules:
          - type: RouteByDirectRegion
            keyFrom:
              - type: header
                name: X-Region
          - type: RouteByKeyConfMatch
            matchMap:
              priority-account: us-east
          - type: RouteByHashRange
```

The reference namespace defaults to the plugin namespace. Cross-namespace references
require a ReferenceGrant in the target namespace unless the target has
`spec.visibility: Cluster` (with reference-grant validation enabled). Missing or denied references are
configuration-unavailable outcomes. Invalid owning resource identity remains a
structural rejection. Settings use the existing EdgionConfigData kind; no separate
ClusterRegionSetting CRD exists.

## Plugin fields

| Field | Default | Meaning |
| --- | --- | --- |
| `clusterRegionRef` | Required | Setting name and optional namespace |
| `failOpen` | `true` | Continue when operational configuration is unavailable |
| `statusOnError` | `503` | HTTP status, 400–599, when failOpen is false |
| `keyGet` | `[]` | Ordered shared key sources |
| `hashKeyGet` | Absent | Ordered sources of a precomputed hash |
| `hashCalc` | Absent | Hash algorithm, modulo and writeback targets |
| `routeRules` | `[]` | Ordered routing rules |
| `routeByKeyConfMatch` | Absent | Shared matchMap and bypassClusterKeys |

There is no plugin-local region table or local-region field. Legacy configuration
fields and the previous setting variant are not compatibility aliases.

## Routing and validation

Rules run in order. Direct-region rules try their own `keyFrom` sources for the first
known region. Key-map rules use the shared `keyGet`; a nonempty shared
`routeByKeyConfMatch.matchMap` takes precedence unless the key is in
`bypassClusterKeys`. Otherwise the rule's own map applies. Hash-range routing is
terminal, including when it passes through without a match.

Hash routing first reads a valid precomputed integer in `[0, modulo)` from
`hashKeyGet`. Otherwise it computes CRC32 or SHA-256 from `keyGet` when `hashCalc`
is configured, then writes the computed value to `writeBack`. The default algorithm
is `crc32`; modulo defaults to 1000 and must be at least 2. Response-header
writeback obeys the shared response-header policy.

Each table row has `name`, `hashRange: [low, high]`, `backendEndpoint`, optional
`tls` (default true), and optional `failoverTo`. Names are nonblank and unique;
the complete table must be nonempty. Closed intervals must be disjoint with
`0 <= low <= high < modulo`; gaps are allowed and pass through. Consumers without
hashCalc still use modulo 1000 for table bounds and precomputed hashes.

Endpoints accept a hostname, IPv4 or bracketed IPv6 with an optional port in
1–65535. URLs and empty hosts are invalid. Omitted ports default to 443 for TLS
and 80 otherwise. Endpoint validation does not bypass post-DNS address checks.
Hostnames resolve through the Gateway's backend DNS answer cache. The resolver
and address family come from the Gateway's `edgion.io/dns-resolver` /
`edgion.io/dns-resolver-ref` / `edgion.io/dns-lookup-family` annotations, then the
EdgionGatewayConfig `dnsResolver` default; the default family is `ipv4Only`, so an
IPv6-only hostname needs `ipv6Only` or `all`. A failed lookup is cached for 1–10
seconds. IP literals are used as written.
The first address that passes the checks, starting from a rotating offset, is
dialed.
Remote routing preserves the original Host, SNI, path and body. A target equal
to the setting's regionName continues through the normal local upstream.

`failoverTo` must name another row. Cycles and self-references are invalid.
Execution follows one hop: for `a -> b -> c`, selecting a routes to b.

## Unavailable configuration and recovery

Missing, disabled, denied, wrong-type or invalid settings, and unusable consumer
hash configuration, invoke the plugin's failure policy before request extraction,
hash writeback or upstream selection. With the default failOpen=true, the plugin
continues to later plugins and normal proxying. With failOpen=false it terminates
with statusOnError. There is no base-table fallback or retained last-good table.
Empty rules still require an available setting, then perform no routing.

This policy applies to readable operational configuration. Invalid YAML, malformed
policy values, missing required fields and structural plugin safety violations do
not become a permissive policy execution. Controller status reports configuration
and reference failures; Gateway independently enforces the runtime contract.

Updates publish the raw setting and validation proof atomically. Each request uses
regionName and regions from the same generation. Full table validation happens
on configuration updates, not on the request path. A subsequent valid update
restores routing without recreating the plugin. A persisted write, Controller
observation and Gateway application are distinct states.

## Diagnostics and dye

Canonical plugin access logs report `setting_reference_unavailable`, `setting_missing`,
`setting_disabled`, `setting_type_mismatch`, `setting_invalid`, or
`hash_modulo_invalid`. The bounded reason label is also recorded in
`edgion_region_route_config_events_total`.

Optional [dye](dye.md) can report success with the final selected region, including
local pass-through. Unavailable configuration records fallback when failOpen=true
or reject otherwise, with cause `configuration-unavailable`. Success requires
nonempty rules. No matching rule or an already-owned upstream jump records no
selection outcome. Dye is disabled by default and follows the shared anti-spoof
and header-mutation policy when explicitly configured.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: regional-routing
  namespace: production
spec:
  requestPlugins:
    - type: RegionRoute
      dye:
        request:
          - name: X-Served-Region     # value omitted => final selected region (on: [success] by default)
        response:
          - name: X-Edgion-Outcome
            on: [fallback]            # value omitted => region_route:fallback:configuration-unavailable
      config:
        clusterRegionRef:
          name: local-region
        failOpen: true
        keyGet:
          - type: header
            name: X-Account-Id
        routeRules:
          - type: RouteByHashRange
```

## Source browsing and single-setting failover

The Controller exposes source-backed setting discovery:

- `GET /api/v1/cluster-region-settings/keys?limit=10&continue=...` returns only
  locators and Cluster/Region identities. An empty matching page can still have
  a continuation token. Disabled settings and readable identities with invalid
  tables remain discoverable; membership comes from `regionName`.
- `GET /api/v1/cluster-region-settings/{namespace}/{name}` reads the current raw
  source, preserving unknown fields. It does not return a processed store object.
- `GET /api/v1/cluster-region-settings/{namespace}/{name}/consumers` evaluates the
  source table against Controller-observed consumer references. `compatibleWithTable`
  is a compatibility result, not a Gateway acknowledgment. `settingEnabled`,
  `tableObservation` and `consumerObservation` identify the separate inputs.

For a single target, POST `{regionName, failoverTo}` to
`/api/v1/cluster-region-settings/{namespace}/{name}/failover/preview`. An empty
`failoverTo` clears the target. The preview includes the intended change, consumer
compatibility and a source `precondition`. Submit the same target fields plus
that precondition to `/api/v1/cluster-region-settings/{namespace}/{name}/failover`.
Changing the intended target should invalidate the UI preview.

The precondition binds canonical source content, UID and resourceVersion, including
unknown fields. It is an equality witness, not an authorization token or a signed
approval of intent. Changed source returns 409; obtain a new preview. An already
matching request returns `modified: false` without writing. Missing, disabled or
invalid settings are not created, enabled or repaired by this operation. The full
proposed table must validate; existing consumer-specific failures are reported
individually and do not introduce a global consumer-health requirement.

A successful write reports `persistence: persisted`; an unchanged result reports
`unchanged`. Controller observation and Gateway application remain `not-confirmed`.
A timeout can occur after persistence: read back before creating another preview,
and do not automatically replay. Filesystem mode serializes in-process mutations
through actual filesystem completion, including cancellation, but does not provide
an atomic compare-and-swap against external file editors. Kubernetes uses native
resourceVersion replacement and etcd uses its modification revision transaction.
Multi-target failover is outside this single-setting operation.
