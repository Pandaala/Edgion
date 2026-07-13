# Federation RBAC

The Controller authorizes each operation that a Federation Center performs on the
Admin API by evaluating a `(verb, kind)` pair derived from the actual matched
route. This document explains the authorization model, the built-in default
policy, the hot-reloadable kill-switch, and common configuration examples.

## How it works

Every Center request (blind-proxy UI calls, sync operations, failover, reload)
passes through the same authorization layer as local admin requests. Center is
assigned a restricted identity (`CenterRestricted`) whose allowed operations are
governed by the `[center].rbac` configuration block. The local admin token maps to
an allow-all (wildcard) policy — Center authorization does not affect it.

Authorization is decided per `(verb, kind)`:

- **verb** — the operation type: `get`, `list`, `list-keys`, `watch`, `create`,
  `update`, `delete`, `failover`, `reload`, `server-info`, `diagnostics`, etc.
- **kind** — the PascalCase resource Kind (e.g., `EdgionConfigData`, `HTTPRoute`,
  `RegionRoute`). Kind matching is **case-sensitive**; use the canonical PascalCase
  spelling. Verbs are case-insensitive.

Authorization is **not per-object**: a `EdgionConfigData` write grant covers all
EdgionConfigData objects, not specific named ones.

## Built-in default (when `rbac` is omitted)

When `[center]` is configured but the `rbac` field is absent, a safe built-in
default policy applies automatically:

| Verb(s) | Kinds | Allowed |
|---------|-------|---------|
| `get`, `list`, `watch`, `list-keys` | All resource kinds **except** `Secret` | Yes |
| `get`, `list`, `watch`, `list-keys` | `Secret` | **No** — credential safety |
| `create`, `update`, `delete` | `EdgionConfigData` | Yes |
| `create`, `update`, `delete` | Any other kind | No |
| `get`, `list`, `failover` | `RegionRoute` (synthetic label, not a resource kind) | Yes |
| `reload`, `server-info`, `diagnostics`, and everything else | Any | No |

Key notes:

- `EdgionTls` **is** readable under the default (TLS configuration, not raw key
  material exposed via Secret). Only the `Secret` kind is excluded.
- The `EdgionConfigData` write grant covers **all** plugin metadata, not only
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
  address: "https://center.example.com:12251"
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
  address: "https://center.example.com:12251"
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
        kinds: [HTTPRoute, GRPCRoute, TCPRoute, Service, EdgionConfigData,
                EdgionGatewayConfig, EdgionTls]
      - verbs: [get, list]
        kinds: [RegionRoute]
```

Note: enumerate concrete kinds explicitly. Do **not** use `kinds: ["*"]` with
read verbs — see [Security notes](#security-notes).

### 3. Region-route and IP-restriction management only

Use when: Center manages region-routing and IP-restriction policy (including
region-route failover) but should have no read access to other resources.

```yaml
center:
  address: "https://center.example.com:12251"
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
        kinds: [EdgionConfigData]
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
  address: "https://center.example.com:12251"
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
  address: "https://center.example.com:12251"
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
        kinds: [EdgionConfigData]
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
  granted kinds. A `EdgionConfigData` write grant covers **all** plugin metadata
  objects, not only specific ones.
- `server-info` and `diagnostics` expose process and configuration details.

### No per-object granularity

The `(verb, kind)` model bounds which operations on which resource types are
allowed. It does **not** restrict which named objects within a kind can be
accessed. A grant of `update:EdgionConfigData` allows updating any EdgionConfigData
object, not only specific ones.

### Canonical kind casing

Kind names in the `rbac` block are matched **case-sensitively** against the
canonical PascalCase Kind name. Lowercase or alias spellings (e.g.,
`edgionconfigdata`, `httpRoute`) silently never match. Always use the canonical
form: `EdgionConfigData`, `HTTPRoute`, `GRPCRoute`, `RegionRoute`, `Service`,
`EdgionTls`, etc.

---

## CLI Token RBAC

CLI tokens let `edgion-cli` (or any HTTP client) call the Controller Admin API
with a **static Bearer token** carrying fine-grained RBAC permissions, without
interactive login. This is the recommended path for CI pipelines, automation
scripts, and service accounts that must call the Admin API non-interactively.

The authorization model is identical to [Center RBAC](#how-it-works): each
token carries a `[center].rbac`-style `FedRbacConfig` (same `verbs × kinds`
taxonomy, same default-deny behavior, same canonical-casing rules). See the
[verb and kind reference](#how-it-works) above for the full list of verbs and
the casing rules.

### Minting a token

```bash
# 1. Generate a new token (32 CSPRNG bytes, URL-safe base64, edg_ prefix)
edgion-cli gen-token
```

Output:

```
Token (use with --token or EDGION_TOKEN):
  edg_<base64url>

SHA-256 (paste into controller cli_tokens[].token_sha256):
  <64 lowercase hex chars>
```

The SHA-256 is computed over the **full `edg_`-prefixed token** — the exact
string sent as `Authorization: Bearer <token>`. This is the value to paste into
the config.

### Controller configuration

Add a `cli_tokens` block to the Controller YAML config (can coexist with `center`
and `admin_token`):

```yaml
cli_tokens:
  # Read-only — safe for monitoring pipelines
  - name: monitor-bot
    token_sha256: "<SHA-256 from gen-token>"
    rbac:
      allow:
        # Enumerate concrete kinds; NEVER use kinds: ["*"] with read verbs.
        - verbs: [get, list]
          kinds: [HTTPRoute, GRPCRoute, TCPRoute, Service,
                  EdgionGatewayConfig, EdgionTls, EdgionConfigData]

  # Scoped read-write — manages plugin metadata and triggers reload
  - name: plugin-manager
    token_sha256: "<SHA-256 from gen-token>"
    rbac:
      allow:
        - verbs: [get, list, watch, create, update, delete]
          kinds: [EdgionConfigData]
        # reload is write-only; kinds: ["*"] is safe here (no read surface)
        - verbs: [reload]
          kinds: ["*"]
```

Hot-reload (no restart required):

```bash
edgion-cli reload
# or: POST /api/v1/reload
```

### Using a CLI token

```bash
# Via flag (long-only; -t is taken by --target)
edgion-cli --token edg_<base64url> get httproute

# Via environment variable (preferred for CI — avoids token in shell history)
export EDGION_TOKEN=edg_<base64url>
edgion-cli get httproute -n production
```

Precedence: `--token` > `EDGION_TOKEN` > cached login token (`~/.edgion/token`,
written by `edgion-cli login --token`).

Validation is lazy: the first real request returns `401 Unauthorized` (unknown
token) or `403 Forbidden` (permission denied). The only public auth endpoint is
`GET /api/v1/auth/status`.

### Security notes

**1. Wide verb grants equal full admin access.**
A token whose `rbac` grants `verbs: ["*"]` on the full admin router — including
`reload`, `failover`, `acme-trigger`, `conf-sync-rotate`, `server-info`, and `diagnostics` — is
effectively equivalent to the allow-all admin token. Grant the minimum set of
verbs the consumer actually needs.

**2. Wildcard read exposes credential material.**
There is no server-side `Secret` backstop on the cli-token path. Using
`kinds: ["*"]` with `get`, `list`, `list-keys`, or `watch` lets the token read
`Secret` and `EdgionTls` private-key material via the blind proxy. Always enumerate
concrete kinds for any read grant. See the [wildcard-read security note](#wildcard-read-exposes-credential-material) above.

**3. Verb and kind strings are canonical-cased.**
Verb strings are matched case-insensitively (`get`, `GET`, and `Get` all work).
Kind strings are matched **case-sensitively** against the canonical PascalCase
Kind name. A miscased kind (e.g. `edgionconfigdata`) silently never matches any
request. Always use canonical PascalCase: `EdgionConfigData`, `HTTPRoute`, `Service`, etc.

**4. Token rotation.**
Replace `token_sha256` in the config and apply via `POST /api/v1/reload`. The
new hash takes effect immediately for all subsequent requests; old tokens are
rejected from that point forward. Distribute the new `edg_` token to the
consumer before or concurrently with the reload.

**5. `--token` and `EDGION_TOKEN` are intended for CI/automation.**
They do **not** touch `~/.edgion/token` (no caching — they are ephemeral
per-invocation). The cached login token is unaffected. Prefer `EDGION_TOKEN`
over `--token` to avoid leaking the token in shell history or process listings.

**6. Migration: no-auth mode was removed.**
Deployments that previously bypassed authentication must now configure an
`admin_token` (or `EDGION_ADMIN_TOKEN`) and/or `cli_tokens`. When neither is set,
the Admin API fails closed with 503. See the
[Controller configuration](configuration/controller-config.md#authentication)
guide for the token-only auth model and the upgrade path.

**7. `list-keys` reveals resource names.**
The `list-keys` verb exposes resource names (and namespace/kind metadata) even
when full `get` or `list` access is not granted. If the existence of resource
names is sensitive, treat `list-keys` as a read grant and enumerate kinds
explicitly rather than using `kinds: ["*"]`.

**8. `EDGION_TOKEN` / `--token` do not use the cached session token.**
These paths bypass `ensure_auth` entirely and do not read from or write to
`~/.edgion/token`. Each invocation is fully stateless with respect to the
local token cache.

**9. Integration-test carve-outs are test-only.**
The `wipe-all` and `debug` verbs, and the integration-testing-mode flag, are
test-only entry points that bypass normal RBAC checks entirely when enabled.
They **must never be enabled in production**. CLI token policy does not govern
these paths — they are guarded by the `--integration-testing-mode` startup flag
on the Controller process, not by the RBAC engine.

### Configuration schema reference

See [Controller Configuration](configuration/controller-config.md#cli-tokens----static-cli-tokens) for
the full field reference and a two-token example.

---

## Related documents

- [Controller Configuration](configuration/controller-config.md)
- [edgion-cli](edgion-cli.md)
