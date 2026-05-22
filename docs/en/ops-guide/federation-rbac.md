# Federation RBAC

The Controller authorizes each operation that a Federation Center performs on the
Admin API by evaluating a `(verb, kind)` pair derived from the actual matched
route. This document explains the authorization model, the built-in default
policy, the hot-reloadable kill-switch, and common configuration examples.

## How it works

Every Center request (blind-proxy UI calls, sync operations, failover, reload)
passes through the same authorization layer as local admin requests. Center is
assigned a restricted identity (`CenterRestricted`) whose allowed operations are
governed by the `[center].rbac` configuration block. Local admin users receive
full (`Superuser`) access — Center authorization does not affect them.

Authorization is decided per `(verb, kind)`:

- **verb** — the operation type: `get`, `list`, `list-keys`, `watch`, `create`,
  `update`, `delete`, `failover`, `reload`, `server-info`, `diagnostics`, etc.
- **kind** — the PascalCase resource Kind (e.g., `PluginMetaData`, `HTTPRoute`,
  `RegionRoute`). Kind matching is **case-sensitive**; use the canonical PascalCase
  spelling. Verbs are case-insensitive.

Authorization is **not per-object**: a `PluginMetaData` write grant covers all
PluginMetaData objects, not specific named ones.

## Built-in default (when `rbac` is omitted)

When `[center]` is configured but the `rbac` field is absent, a safe built-in
default policy applies automatically:

| Verb(s) | Kinds | Allowed |
|---------|-------|---------|
| `get`, `list`, `watch`, `list-keys` | All resource kinds **except** `Secret` | Yes |
| `get`, `list`, `watch`, `list-keys` | `Secret` | **No** — credential safety |
| `create`, `update`, `delete` | `PluginMetaData` | Yes |
| `create`, `update`, `delete` | Any other kind | No |
| `get`, `list`, `failover` | `RegionRoute` (synthetic label, not a resource kind) | Yes |
| `reload`, `server-info`, `diagnostics`, and everything else | Any | No |

Key notes:

- `EdgionTls` **is** readable under the default (TLS configuration, not raw key
  material exposed via Secret). Only the `Secret` kind is excluded.
- The `PluginMetaData` write grant covers **all** plugin metadata, not only
  region-route or IP-restriction records. This is a known limitation of the
  kind-level granularity.
- `RegionRoute` is a synthetic classifier label (not a `ResourceKind` variant).
  `get` and `list` cover the aggregate read endpoints
  (`GET /api/v1/cluster-region-routes`, `GET /api/v1/service-region-routes`);
  `failover` covers the POST failover endpoint. These three verbs are kept in a
  separate rule from the ResourceKind read rule because `RegionRoute` is not part
  of the exhaustiveness gate.
- `reload` is **not** included in the default. If Center must trigger config
  reloads, supply an explicit `rbac` block.

When an explicit `rbac` block is present it **fully overrides** the built-in
default. There is no merging.

## `center.enabled` kill-switch

```yaml
center:
  enabled: false   # default: true
```

Setting `enabled: false` stops the federation client entirely without a process
restart. Use `POST /api/v1/reload` to hot-apply the change after editing the
config file. The reload also re-reads the `rbac` block and applies the new policy
to all subsequent Center requests without disconnecting and reconnecting the client
(unless `enabled` or `address` changed).

## Authorization examples

### 1. Default (omit `rbac`)

Covers Center's automated operations (IP-restriction sync, region-route sync,
failover) and read access to all resource kinds except `Secret`. Suitable for
most standard deployments.

```yaml
center:
  address: "https://center.example.com:50052"
  name: "ctrl-1"
  cluster: "prod"
  security:
    active: primary
    certs:
      - name: primary
        cert: certs/grpc/primary/client.crt
        key: certs/grpc/primary/client.key
        ca: certs/grpc/primary/ca.crt
  # rbac omitted — built-in default applies
```

### 2. Strict read-only

Use when: Center is allowed to observe resources for dashboard/audit purposes
only, with no write access and no failover.

```yaml
center:
  address: "https://center.example.com:50052"
  name: "ctrl-1"
  cluster: "prod"
  security:
    active: primary
    certs:
      - name: primary
        cert: certs/grpc/primary/client.crt
        key: certs/grpc/primary/client.key
        ca: certs/grpc/primary/ca.crt
  rbac:
    allow:
      - verbs: [get, list, watch]
        kinds: [HTTPRoute, GRPCRoute, TCPRoute, Service, PluginMetaData,
                RegionRoute, EdgionGatewayConfig, EdgionTls]
```

Note: enumerate concrete kinds explicitly. Do **not** use `kinds: ["*"]` with
read verbs — see [Security notes](#security-notes).

### 3. Region-route and IP-restriction management only

Use when: Center manages region-routing and IP-restriction policy but should have
no read access to other resources and no failover capability.

```yaml
center:
  address: "https://center.example.com:50052"
  name: "ctrl-1"
  cluster: "prod"
  security:
    active: primary
    certs:
      - name: primary
        cert: certs/grpc/primary/client.crt
        key: certs/grpc/primary/client.key
        ca: certs/grpc/primary/ca.crt
  rbac:
    allow:
      - verbs: [get, list, watch, create, update, delete]
        kinds: [PluginMetaData]
      - verbs: [failover]
        kinds: [RegionRoute]
```

### 4. Full control — with security warning

Use when: **trusted internal deployments only, where the Center operator and the
Controller operator are the same team and you accept the risks listed below.**

> **WARNING — read the [Security notes](#security-notes) before enabling this.**
> `kinds: ["*"]` with read verbs exposes `Secret` and `EdgionTls` private-key
> material. `verbs: ["*"]` lets Center mutate any resource or trigger reloads.
> `"*":"*"` is equivalent to granting Center full control of the Controller.

```yaml
center:
  address: "https://center.example.com:50052"
  name: "ctrl-1"
  cluster: "prod"
  security:
    active: primary
    certs:
      - name: primary
        cert: certs/grpc/primary/client.crt
        key: certs/grpc/primary/client.key
        ca: certs/grpc/primary/ca.crt
  rbac:
    allow:
      - verbs: ["*"]
        kinds: ["*"]
```

### 5. Custom least-privilege

Use when: you have a specific set of Center features enabled and want to grant
exactly what is needed — nothing more.

```yaml
center:
  address: "https://center.example.com:50052"
  name: "ctrl-1"
  cluster: "prod"
  security:
    active: primary
    certs:
      - name: primary
        cert: certs/grpc/primary/client.crt
        key: certs/grpc/primary/client.key
        ca: certs/grpc/primary/ca.crt
  rbac:
    allow:
      # Center sync: read and write plugin metadata
      - verbs: [get, list, watch, create, update, delete]
        kinds: [PluginMetaData]
      # Regional failover
      - verbs: [failover]
        kinds: [RegionRoute]
      # Dashboard read: enumerate specific kinds only
      - verbs: [get, list]
        kinds: [HTTPRoute, GRPCRoute, Service, EdgionGatewayConfig]
      # Allow Center to trigger config reload
      - verbs: [reload]
        kinds: ["*"]
```

## Security notes

### Wildcard read exposes credential material

There is **no server-side Secret backstop** on the federation path. When
`kinds: ["*"]` is combined with `get`, `list`, or `watch`, Center can read the
contents of `Secret` and `EdgionTls` resources — including private keys and
certificates — via the blind proxy.

The built-in default avoids this by excluding the `Secret` kind. Any explicit
`rbac` block is the operator's full responsibility.

**Always enumerate concrete kinds for read grants.** Use example 2 or 5 as a
template rather than `kinds: ["*"]`.

### Wide write and reload grants

- `verbs: ["*"]` or an explicit `reload` grant lets Center trigger a Controller
  config reload, which re-reads and applies the `[center]` block from disk.
  Center **cannot** modify the on-disk YAML (no config-writing route exists), so
  the worst case is forcing re-application of the operator's own file.
- Write grants (`create`, `update`, `delete`) let Center mutate resources of the
  granted kinds. A `PluginMetaData` write grant covers **all** plugin metadata
  objects, not only specific ones.
- `server-info` and `diagnostics` expose process and configuration details.

### No per-object granularity

The `(verb, kind)` model bounds which operations on which resource types are
allowed. It does **not** restrict which named objects within a kind can be
accessed. A grant of `update:PluginMetaData` allows updating any PluginMetaData
object, not only specific ones.

### Canonical kind casing

Kind names in the `rbac` block are matched **case-sensitively** against the
canonical PascalCase Kind name. Lowercase or alias spellings (e.g.,
`pluginmetadata`, `httpRoute`) silently never match. Always use the canonical
form: `PluginMetaData`, `HTTPRoute`, `GRPCRoute`, `RegionRoute`, `Service`,
`EdgionTls`, etc.

## Reference

- Controller configuration schema: `skills/02-features/02-config/00-controller-config.md`
- Sample configuration file: `config/edgion-controller.yaml`
- Design specification: `docs/superpowers/specs/2026-05-21-fed-center-rbac-design.md`
