---
name: controller-admin-api
description: edgion-controller HTTP listeners — Admin API (:12101), probe listener (:12100), and metrics listener (:12190) — full endpoint list, AdminState, CRUD, and ConfigServer endpoints.
---

# Admin API (:12101)

The Controller exposes three HTTP listeners:

| Listener | Config field | Default | Serves |
|----------|-------------|---------|--------|
| Probe | `probe_listen` | `0.0.0.0:12100` | `/health`, `/ready` |
| Metrics | `metrics_listen` | `0.0.0.0:12190` | `/metrics` |
| Admin | `admin_listen` | `127.0.0.1:12101` (loopback; set `0.0.0.0` + `allow_admin_ips` for cluster/remote access) | `/api/v1/*`, `/configclient` |

The Admin API listens on port 12101 (default) via Axum, used by the edgion-cli command-line tool and operations workflows. `/health` and `/ready` are served exclusively on the **probe listener** (`:12100`); `/metrics` is served exclusively on the **metrics listener** (`:12190`). Neither endpoint is available on `:12101`.

## Router Composition (token-only middleware onion)

The Admin API server is assembled in `edgion-controller/src/api/mod.rs::serve_with_shutdown`. The runtime middleware order (outermost first) is:

```text
IP allowlist → cache_control_no_store → require_token_configured_gate (503) → cli_token_pre_authn → [authz_layer route_layer] → handler
```

1. **`business`** — the per-component business routes from `create_admin_router`, wrapped with `authz_layer` as a `route_layer` (matched routes only, never the 404 fallback).
2. **`/api/v1/auth/status`** — merged *after* the `authz_layer` `route_layer` so it stays reachable without a token (CLI `ensure_auth` requires it at all times).
3. **`cli_token_pre_authn`** — the sole authn layer: a recognized Bearer token injects `Role::Restricted`; an absent/unknown token injects nothing.
4. **`require_token_configured_gate`** — returns `503` on business routes when no token (admin or cli) is configured; skips the static public paths.
5. **`cache_control_no_store`** — blanket `Cache-Control: no-store`.
6. **IP allowlist** — outermost; mounted only when `allow_admin_ips` is non-empty.

Authentication is **token-only** (Istio-style pre-shared bearer; Bearer header only). The full model — `Role`/`Verb`/`CenterPolicy`, the local admin token, and federation mTLS — is in [11-authentication-authorization.md](11-authentication-authorization.md).

**CORS note**: the Admin API does not include a CORS layer. For local debugging, `edgion-dashboard`'s Vite `server.proxy` handles it (see `edgion-dashboard/vite.config.ts`); in production, the dashboard is embedded into the Controller and served from the same origin. If a separate-origin deployment is required in the future, introduce a precise origin allowlist (corresponds to the H1 fix record).

## AdminState — Shared State

All handlers share state through Axum's `State<Arc<AdminState>>`:

```rust
pub struct AdminState {
    pub conf_mgr: Arc<ConfMgr>,
    pub schema_validator: Arc<SchemaValidator>,
}
```

`AdminState` provides two layers of data-access methods:

| Method prefix | Data source | Purpose |
|---------------|-------------|---------|
| `center_*` | ConfCenter (storage layer) | Used by `/api/v1/...` endpoints; reads/writes the underlying storage directly |
| `cache_*` | ConfigSyncServer (ServerCache) | Used by `/configserver/...` endpoints; reads the processed cache |

Helper methods:

- `config_sync_server()` — get an `Arc<ConfigSyncServer>`; returns `503` if not ready
- `is_ready()` — check whether ConfigSyncServer exists
- `runtime()` — access the ConfCenter's `ControllerRuntime` for diagnostic/effective views
- `is_k8s_mode()` — whether the mode is Kubernetes (in K8s mode local schema validation is skipped)

## Full Endpoint List

### Health and Status (Probe Listener :12100)

The following endpoints are served on the **probe listener** (`:12100`, `probe_listen`), **not** on the Admin API port:

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Liveness probe. Returns `200` as soon as the service starts; response body `{"success":true,"data":"OK"}` |
| GET | `/ready` | Readiness probe. Returns `200` once ConfigSyncServer is ready, otherwise `503` |

### Admin API Status (Admin Listener :12101)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/server-info` | Get the current `server_id` and ready state. A process restart or resource-epoch replacement changes `server_id` |
| GET | `/api/v1/access` | Return the **caller's own** effective access snapshot. Authenticated-only; see below |

#### `GET /api/v1/access` — effective access introspection

Lets an authenticated client (Center's UI, `edgion-cli`) discover what it may
actually do, instead of probing endpoints and inferring from `403`s. Handler:
`api/access_handlers.rs`; snapshot builder: `authz/access.rs`.

Response `data` shape:

```json
{
  "schemaVersion": 1,
  "revision": "sha256:...",
  "resources": [ { "kind": "HTTPRoute", "scope": "namespaced", "verbs": ["get", "list"] } ],
  "operations": [ { "name": "regionRoute.list", "allowed": true } ]
}
```

- **Bounded**: `resources` and `operations` have fixed cardinality drawn from a
  compile-time catalog. A wildcard policy (`verbs: ["*"]`, `kinds: ["*"]`) is
  *expanded against that catalog* — it never emits a kind or verb outside it.
  The catalog is held to `ResourceKind` by an exhaustive-match compile guard in
  `authz/access.rs`.
- **`revision`** is a SHA-256 over the canonical `(schemaVersion, resources, operations)`
  serialization — stable for identical access, and it changes when the policy is
  changed at process restart, so a client can cheaply detect that its permissions moved.
- **Self-only**: it reports the `Role` already attached to *this* request. There
  is no way to ask about another identity.
- **Auth**: missing/unknown token → `401` (the route is not in the public
  skip-set). It carries the inherent `access-self` verb, which no RBAC rule can
  grant or revoke — see
  [11-authentication-authorization.md](11-authentication-authorization.md).

### Namespaced Resource CRUD

All endpoints read/write the **ConfCenter storage layer**. The request body supports JSON and YAML (JSON parsing is tried first).

The shared resource parser and source-version gate log fixed failure stages and
optional numeric line/column coordinates, never parser error Display or authored
values/key paths. Rejection still returns HTTP 400 before persistence (ARCH-140).

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/namespaced/{kind}` | List all resources of a kind across all namespaces |
| GET | `/api/v1/namespaced/{kind}/{namespace}` | List resources of a kind in the given namespace |
| POST | `/api/v1/namespaced/{kind}/{namespace}` | Create a namespaced resource |
| GET | `/api/v1/namespaced/{kind}/{namespace}/{name}` | Get a single namespaced resource |
| PUT | `/api/v1/namespaced/{kind}/{namespace}/{name}` | Update a namespaced resource |
| DELETE | `/api/v1/namespaced/{kind}/{namespace}/{name}` | Delete a namespaced resource |

The `{kind}` path parameter is the resource type name (e.g., `HTTPRoute`, `Gateway`); it is matched case-insensitively via `parse_kind()`, and unknown kinds return `400`.

In non-K8s mode, create and update operations:
1. Run JSON Schema validation via `SchemaValidator` (skipped in K8s mode, which is handled by the API Server)
2. Automatically call `next_resource_version()` to assign an incrementing `resourceVersion`

Update **and delete** endpoints support optimistic concurrency through a strong
`If-Match: "<resourceVersion>"` header. `expected_resource_version()`
(`api/common.rs`) resolves the precondition in this order: the `If-Match`
header, then — for updates only — the request body's `metadata.resourceVersion`
as a compatibility fallback (`DELETE` has no body, so the header is its only
form), then none. A mismatch returns `409 Conflict`. Weak (`W/"..."`) and
multi-value ETags are rejected with `400`. File-system comparison runs inside
the per-resource lock, etcd combines the comparison with its mod-revision
transaction, and Kubernetes compares the fetched object before a replace that
retains the fetched native resourceVersion.

**The precondition is opt-in, and a write without one is last-writer-wins.**
State this plainly to callers rather than treating it as a compatibility
concession: a `PUT` or `DELETE` that carries neither an `If-Match` header nor a
body `metadata.resourceVersion` is an **unconditional replace of every field the
caller owns** (`labels`, `annotations`, `spec`, top-level operator fields).
`merge_operator_update_*` takes `apiVersion`, `kind`, and `status` from the
stored object and preserves server-created metadata — except that the non-K8s
backends persist the request's `resourceVersion` and `generation`, per the
freshly-allocated-version rule below. An update another client committed after
this caller performed its `GET` is silently discarded, and this caller still
receives `200`/`204`. Nothing reports the loss to either party.

Two things that look like protection are not. `If-Match: *` is unconditional
too; it means only "must exist", and existence is already enforced by the
update/delete path itself (`ConfWriterError::NotFound` → `404`). And
`api/resource_locks.rs` only serialises writers so a read-modify-write cannot
interleave, which makes a lost update *deterministic* rather than *detected* —
it is also taken on just two paths (the generic `EdgionConfigData` `PUT` arm of
`update_namespaced` and the RegionRoute failover RMW), not on every write.

A `409` is still reachable without a client precondition — **on `PUT` only** —
but it comes from the backend's own get→write window, which is far narrower than
the caller's cross-request read→modify→write: etcd commits through
`txn_put_if_mod_revision` against the revision it just read, and the Kubernetes
replace carries the fetched native `resourceVersion` so the API server applies
its own optimistic concurrency. The file-system backend has no CAS of its own
(read, compare, write under `update_lock` — it enforces a *supplied* precondition
and nothing more). No backend *requests* a delete
precondition when the caller sent none, so none generates a `409` on that path
(etcd takes the plain-delete branch and maps a zero-delete to `NotFound`,
Kubernetes passes `preconditions: None`, and file-system skips the pre-read) —
though in K8s mode `map_kube_error` forwards any API-server `409` verbatim, so
that is a promise about what Edgion asks for, not about what the API Server can
return. So a precondition-free `PUT` may still see a `409` it did not ask for,
and no caller may read the absence of one as "my update was the only one".

Detection of the caller's *own* conflict is therefore the caller's
responsibility — it must send the precondition from its own `GET`.
`EdgionCenter`'s dedicated `EdgionConfigData` write path always does (`If-Match`
is in the federation proxy header allowlist, and the path refuses to dispatch
without a version); its dashboard's generic resource `PUT` sends one whenever
the edited document carries a `resourceVersion`. `edgion-cli apply`
(`commands/apply.rs`, which performs a `GET` but uses only its status code),
`edgion-cli delete`, the `edgion-tests` `apply_yaml` helper
(`client/framework.rs`), an ad-hoc `curl`, and any GitOps tool issuing a plain
`PUT` send none, and are last-writer-wins by construction. (A hand-written
manifest that happens to carry `metadata.resourceVersion` makes such a `PUT`
conditional through the body fallback — usually by accident.)

This is deliberate parity with the Kubernetes API, whose update verb is likewise
unconditional when `metadata.resourceVersion` is empty (upstream behavior, not
something this repository asserts or tests). Requiring the
precondition here would make this API stricter than the API it mirrors, and in
K8s mode the guarantee would be unenforceable anyway because `kubectl apply`
writes the same CRD without passing through this API at all. See
[`../../04-review/admin-api/intentional-design/admin-api-write-precondition-opt-in.md`](../../04-review/admin-api/intentional-design/admin-api-write-precondition-opt-in.md)
for the full decision.

In non-K8s mode every persisted write must carry a **freshly allocated**
`resourceVersion` (`next_resource_version()`), because those backends store the
request's value verbatim. Writing a resource back at the version it was read at
would freeze the version across a mutation, and a client still holding that
version could then satisfy an `If-Match` precondition and silently overwrite
the change. This applies to *every* server-side read-modify-write path, not
just `PUT` — including the RegionRoute failover endpoint. K8s mode assigns the
version server-side and must not be touched.

Successful `POST` creates respond with **`201 Created`** and a `Location` header pointing at the new resource. `PUT` updates continue to use `200 OK` with the `ApiResponse::success(...)` body. Successful `DELETE` returns **`204 No Content`** with an empty body. Failed creates still surface as the mapped HTTP error (see "Error Mapping" below) and carry no `Location` header.

**`ApiResponse<T>` carries an optional `warnings` field**: `{ "success": bool, "data": Option<T>, "error": Option<String>, "warnings": Vec<String> }`, with `warnings` omitted from the JSON entirely when empty (`skip_serializing_if = "Vec::is_empty"`) — every pre-existing call site is unaffected. It is populated on `EdgionPlugins`, `EdgionStreamPlugins`, `EdgionConfigData`, and `LinkSys` writes through this Admin API when the submitted document contains a key the schema does not recognize (the same finding class as `GET /api/v1/diagnostics/unknown-fields` above). This is the **only** ingress with no second chance to report the finding: `create_namespaced` / `update_namespaced` persist a typed round-trip (`serde_json::to_string(&obj)`), which strips unknown keys before they are ever written to storage — the FS loader, etcd loader, and Kubernetes watcher hook that catch the finding at every other ingress never get to see a document that was cleaned before it reached them. The write still succeeds (`success: true`, unaffected status code) — this is advisory, not a rejection. For `EdgionConfigData`, typed variants are checked while `Misc.config` stays deliberately open. For LinkSys, declared map and arbitrary-JSON values remain open while unknown siblings in typed provider configurations are reported.

### Cluster-scoped Resource CRUD

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/cluster/{kind}` | List all cluster-scoped resources of a kind |
| POST | `/api/v1/cluster/{kind}` | Create a cluster-scoped resource |
| GET | `/api/v1/cluster/{kind}/{name}` | Get a single cluster-scoped resource |
| PUT | `/api/v1/cluster/{kind}/{name}` | Update a cluster-scoped resource |
| DELETE | `/api/v1/cluster/{kind}/{name}` | Delete a cluster-scoped resource |

Cluster-scoped resources (such as GatewayClass) have no namespace dimension. The handler validates internally via `is_kind_cluster_scoped()` whether the kind is actually cluster-scoped, and returns `400` if it does not match.

### Metadata-only Keys (list-keys-api card 01)

Parallel read-only endpoints returning `ResourceKey` (`apiVersion` / `kind` /
`metadata` only — no `spec` / `status`). The
`/api/v1/{cluster,namespaced}/...` family above remains the full-resource CRUD
API used by Controller clients; `/api/v1/keys/...` is a metadata-only listing
surface. The two families differ by payload and purpose, not by legacy/current
status.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/keys/cluster/{kind}` | List cluster-scoped resource keys |
| GET | `/api/v1/keys/namespaced/{kind}` | List keys across all namespaces |
| GET | `/api/v1/keys/namespaced/{kind}/{namespace}` | List keys in one namespace |

Query params: `limit=N` (page size) and `continue=<token>` (K8s-style cursor). Response carries `continue_token` for the next page (`null` on the last page).

### Effective Views, Operations, and Diagnostics

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/v1/region-routes/effective` | Effective region-route view |
| POST | `/api/v1/cluster-region-routes/failover` | Patch region failover state |
| POST | `/api/v1/services/acme/{namespace}/{name}/trigger` | Trigger ACME processing |
| POST | `/api/v1/services/conf-sync-ca/rotate` | Trigger conf_sync CA rotation |
| GET | `/api/v1/diagnostics/nested_ext_ref_usage` | ExtensionRef usage diagnostics |
| GET | `/api/v1/diagnostics/conf-conflicts` | Configuration-conflict diagnostics |
| GET | `/api/v1/diagnostics/unknown-fields` | Fleet-wide unknown-config-key findings (see below) |
| POST | `/api/v1/validate/{kind}` | Side-effect-free validation verdict for a submitted document (see below) |

Test mode additionally mounts authenticated `/api/v1/debug/store-stats` and
`/api/v1/admin/wipe-all`; they are not production routes.

#### `GET /api/v1/diagnostics/unknown-fields` — unknown config keys, fleet-wide

Answers "did anything applied to this Controller contain a key the schema does not
recognize?" across `EdgionPlugins`, `EdgionStreamPlugins`, typed `EdgionConfigData` variants,
and LinkSys provider configurations — resources whose
`config` subtree is preserved by the CRD and therefore not pruned
by the API server or caught by `deny_unknown_fields` (see
`skills/00-design-rules.md` and
`skills/04-review/resources/compatibility-constraints/serde-ignored-blind-spots-schema-walk-instead.md`
for the full rationale). Requires the standard Admin API bearer token; no extra scope.

Backed by `observe::unknown_fields_registry`, a keyed registry read from the same typed
`RawStore` each conf_center mode already wires up for status rendering — not
`center_list_resources`, since a finding lives in a field the CRD/wire schema deliberately
never carries (`#[serde(skip)] #[schemars(skip)] unknown_fields: Vec<String>` on
`EdgionPluginsSpec`, `EdgionStreamPluginsSpec`, `EdgionConfigDataSpec`, or `LinkSysSpec`).

```json
{
  "success": true,
  "data": {
    "total": 2,
    "truncated": false,
    "items": [
      {
        "kind": "EdgionPlugins",
        "namespace": "default",
        "name": "auth-chain",
        "paths": ["requestPlugins[0].config.denys"]
      }
    ]
  }
}
```

`total` is the pre-cap finding count across the fleet; `items` is capped at 100 entries
(`truncated: true` when the cap bites, reported honestly rather than silently dropping
entries). Each item's path list is also capped with a visible deterministic marker. This endpoint never rejects anything itself — it is the query surface an
operator's own blocking gate (CI, a `ValidatingAdmissionWebhook`) is meant to be built on;
see `POST /api/v1/validate/{kind}` below for the synchronous per-document equivalent.

#### `POST /api/v1/validate/{kind}` — side-effect-free validation verdict

A synchronous, read-only check against a document exactly as it would be applied — no
version bump, nothing persisted, no store touched. Body is the resource document (YAML or
JSON, same auto-detection as the write endpoints). Exists so an operator who wants
**blocking** (which the Controller itself deliberately never does for a config-key mismatch
— see "Policy: warn, never block" in the plugin resource pages) can build their own
admission webhook, CI check, or GitOps pre-merge gate on top of it.

```json
POST /api/v1/validate/EdgionPlugins
<document body>

{
  "success": true,
  "data": {
    "ok": false,
    "unknownFieldCheck": "performed",
    "unknownFields": ["requestPlugins[0].config.denys"],
    "schemaErrors": [],
    "validationErrors": []
  }
}
```

Steps run in order: (1) `extract_resource_metadata` on the raw body, same early sanity check
the write path performs; (2) a typed parse with `update_version: false`; (3) the CRD-schema
validator, forced on regardless of conf_center mode (a validation *query* should answer
fully regardless of who else validates in production); (4) the unknown-field schema walk on
the raw document (`edgion-resources::schema_check::unknown_key_paths`) — the one check no
other layer performs, because the CRD preserves these `config`
`x-kubernetes-preserve-unknown-fields`. `validationErrors` additionally carries
`EdgionPlugins::validate_plugins()` output when `kind == EdgionPlugins` — the same
preparse business-rule check the conf_sync apply path runs, minus the DSL VM init step
(which needs the gateway plugin runtime and cannot run side-effect-free); it stays empty for
every other kind.

`unknownFieldCheck` is `"performed"` for `EdgionPlugins`, `EdgionStreamPlugins`,
`EdgionConfigData`, and `LinkSys`, and `"unsupported"` for every other kind. This discriminator exists so a caller can never misread an empty
`unknownFields: []` as "checked, nothing found" when it actually means "this kind is not
covered by the walk at all".

Backend behaviour:

- **K8s**: `Api::list_metadata` (server-side `PartialObjectMetadataList` projection). The annotation `kubectl.kubernetes.io/last-applied-configuration` is stripped on the way out; other labels/annotations pass through. Native server-side pagination via `continue` token.
- **FS / Etcd**: trait-default projection — delegates to `get_list_by_kind*` (which already serves the informer's full-list path) and projects each `ConfEntry` to a `ResourceKey` by parsing only the `metadata` block. Strips the same `last-applied-configuration` annotation. **No pagination** (`continue_token` is always `null`); the existing `get_list_by_kind*` returns the full set, so the keys endpoint inherits that shape. If a future workload demands real FS/Etcd pagination, extend `get_list_by_kind*`'s own pagination support — the default `list_keys_*` impl transparently propagates the resulting `continue_token`.
- Stale / expired `continue` token (K8s 410 Gone only) maps to **HTTP 410**; clients must restart pagination from the first page.

Call paths land in `AdminState::center_list_keys{,_namespaced}` → `CenterApi::list_keys_by_kind{,_ns}` → `ConfCenter` backend.

### ConfigServer Endpoints

Used by `edgion-cli --target server`; reads from the **ConfigSyncServer cache layer** (ServerCache). The returned data is the processed snapshot produced by ResourceProcessor, in a format compatible with Gateway's `/configclient/` API.

| Method | Path | Query parameters | Description |
|--------|------|------------------|-------------|
| GET | `/configserver/{kind}/list` | none | List all cached resources of a kind |
| GET | `/configserver/{kind}` | `namespace` (optional), `name` (required) | Get a single cached resource. Namespaced resources require namespace |

Response format examples:

```json
// list response
{"success": true, "data": [...], "count": 5}

// single-resource response
{"success": true, "data": {"metadata": {...}, "spec": {...}}}
```

Error responses use standard HTTP status codes (no JSON body):

| Condition | HTTP status |
|-----------|-------------|
| `name` query parameter absent | 400 Bad Request |
| `kind` path segment unrecognised | 400 Bad Request |
| Resource not found in cache | 404 Not Found |
| ConfigSyncServer not yet ready | 503 Service Unavailable |

### ACME Service

| Method | Path | Description |
|--------|------|-------------|
| POST | `/api/v1/services/acme/{namespace}/{name}/trigger` | Manually trigger issuance/renewal for the specified ACME resource. Resets the retry counter and re-evaluates the resource |

## Health and Readiness Semantics

Both probes are served on the **probe listener** (`:12100`, `probe_listen`). Use `httpGet.port: 12100` in your Kubernetes pod spec.

| Probe | Endpoint | Listener | Semantics | K8s usage |
|-------|----------|----------|-----------|-----------|
| Liveness | `/health` | `:12100` (`probe_listen`) | Returns 200 as long as the process is alive | `livenessProbe` — failing causes the Pod to restart |
| Readiness | `/ready` | `:12100` (`probe_listen`) | Returns 200 after ConfigSyncServer initialization is complete and all Processors are ready | `readinessProbe` — when not ready, the pod is removed from the Service endpoints |

Example Kubernetes probe configuration:

```yaml
livenessProbe:
  httpGet:
    path: /health
    port: 12100
readinessProbe:
  httpGet:
    path: /ready
    port: 12100
```

Readiness is determined via `AdminState::is_ready()` → `ConfMgr::is_ready()` →
`ConfCenter::is_ready()`. It requires both a ConfigSyncServer and all expected
processors to be ready; server existence alone is insufficient.

## Write-path readiness gate

The router applies `require_ready_middleware` (in `edgion-controller/src/api/mod.rs`) as a `route_layer` over the resource-CRUD write endpoints:

- `POST /api/v1/cluster/{kind}` / `PUT /api/v1/cluster/{kind}/{name}` / `DELETE /api/v1/cluster/{kind}/{name}`
- `POST /api/v1/namespaced/{kind}/{namespace}` / `PUT /api/v1/namespaced/{kind}/{namespace}/{name}` / `DELETE /api/v1/namespaced/{kind}/{namespace}/{name}`

While the controller is initializing or recovering (`is_ready() == false`), these endpoints respond with `503 Service Unavailable` instead of entering `parse_resource_and_update_version` and `ConfCenter::create_one / update_one / delete_one`. Without this gate, writes accepted during the LIST + cache-build window bypass cache consistency and (in non-K8s mode) race `update_resource_version` against the init flow's `resourceVersion` sequencing.

Endpoints intentionally not gated:

- `GET` endpoints on `/api/v1/...` (reads may return partial data during init — explicit operator/debug choice).
- `/api/v1/auth/*` — auth must stay reachable. (`/health`, `/ready`, `/metrics` are on separate listeners and are not routed through the Admin API router at all.)
- `/api/v1/services/acme/.../trigger` — notify-only, does not touch the storage layer.
- `/api/v1/diagnostics/...` and `/api/v1/server-info` — read-only diagnostic surface.
- `/configserver/...` — read-only cache surface; already returns 503 via `config_sync_server()` when not ready.

**Known gaps** (write paths that touch `ConfCenter` but are not yet covered by this gate, tracked separately):

- `POST /api/v1/cluster-region-routes/failover` — `region_route_handlers::patch_edgion_config_data_failover` calls `ConfCenter::update_one` on `EdgionConfigData` but does not flow through `parse_resource_and_update_version`. Shares the same init-race risk; out of scope for the initial fix.
- `POST /api/v1/admin/wipe-all` — test-mode-only (gated by `is_test_mode()`), calls `delete_one` per kind. No production exposure.

## Interaction with edgion-cli

edgion-cli supports three `--target` modes that map to different endpoints:

| target mode | Endpoints | Data source | Description |
|-------------|-----------|-------------|-------------|
| `center` | `/api/v1/namespaced/...` and `/api/v1/cluster/...` | ConfCenter storage layer | View/modify the original configuration |
| `server` | `/configserver/...` | ConfigSyncServer cache | View processed configuration snapshots (matches what Gateway sees) |
| `client` | Gateway's `/configclient/...` | Gateway local cache | View the configuration actually used by Gateway |

These three views help operators locate problems on the configuration sync pipeline: present in storage but missing from the cache means the Processor is failing; present in the cache but missing from Gateway means gRPC sync is failing.

## Error Mapping

Mapping from `ConfWriterError` to HTTP status codes:

| ConfWriterError variant | HTTP status |
|-------------------------|-------------|
| `NotFound` | 404 Not Found |
| `AlreadyExists` | 409 Conflict |
| `ValidationError` | 400 Bad Request |
| `PermissionDenied` | 403 Forbidden |
| `Conflict` | 409 Conflict |
| `ParseError` | 400 Bad Request |
| `IOError` | 500 Internal Server Error |
| `KubeError` | 500 Internal Server Error |
| `EtcdError` | 500 Internal Server Error |
| `InternalError` | 500 Internal Server Error |
| `NotSupported` | 501 Not Implemented |
| `StalePagination` | 410 Gone |

## Authentication System

The Admin API uses **token-only** authentication: one operator admin token and optional scoped CLI tokens, all supplied as `Authorization: Bearer <token>`. There is no username/password login, OIDC admin, cookie, or session.

> This section covers **authentication** (who you are) only. For **authorization** (the `(verb, kind)` RBAC engine, the `Role` model, cli tokens, the local admin token, and the full middleware onion), see [11-authentication-authorization.md](11-authentication-authorization.md).

### Authentication Middleware (cli_token_pre_authn)

Module location: `edgion-controller/src/api/cli_token_middleware.rs` — the sole Admin API authn layer.

Request authentication order:

1. Static public paths (`/health`, `/ready`, `/metrics`, `/api/v1/auth/status`) → pass through (probes are also served on dedicated listeners).
2. Read the `Authorization: Bearer <token>` header (case-insensitive; no cookie).
3. Hash the token (`sha256_hex`) and look it up in the `Authorizer` token map (`cli_lookup`). On a hit, inject `Role::Restricted` + `CliTokenIdentity`.
4. On a miss or no bearer, inject nothing — `authz_layer` returns `401` with `WWW-Authenticate: Bearer`. A recognized token denied by RBAC gets `403`.

### Authentication Endpoints

The only auth endpoint is the public status probe; login/logout/me are gone.

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/api/v1/auth/status` | not required (public) | Return `{ auth_required: true, mode: "token", status }`, where status is `Ready` when any token is registered or `Initializing` when none is configured |

### Admin Token Bootstrap (Controller)

There is no credential auto-generation. The operator supplies a single admin token; when none is configured, the server starts but all Admin API business routes return `503` fail-close (via `require_token_configured_gate`) with a startup WARN.

The token is read from the following sources (highest precedence first):

1. **Env var** `EDGION_ADMIN_TOKEN`, injected via `valueFrom.secretKeyRef` from the `edgion-admin-credentials` Secret (key `token`; recommended for Kubernetes).
2. **`admin_token`** config field (a Secret-mounted config file).

A blank/whitespace token is treated as not configured. The admin token maps to an allow-all wildcard RBAC policy and is installed at startup by `Authorizer::apply`. When nothing is configured: server starts, business routes return `503`, and `/health` / `/ready` / `/metrics` / `/api/v1/auth/status` remain accessible.

See `skills/02-features/02-config/03-auth-bootstrap.md` for the full matrix and migration guide.

### Module Locations

| Module | Path |
|--------|------|
| Token authn middleware (`cli_token_pre_authn`) | `edgion-controller/src/api/cli_token_middleware.rs` |
| Authz engine + admin-token entry (`Authorizer`) | `edgion-controller/src/authz/mod.rs` |
| Admin token resolution (`EDGION_ADMIN_TOKEN` / `admin_token`) | `edgion-controller/src/cli/mod.rs` |

## Startup Modes

The Admin API supports two startup modes:

```rust
// regular startup
api::serve(conf_mgr, schema_validator, addr).await?;

// startup with graceful shutdown
api::serve_with_shutdown(conf_mgr, schema_validator, addr, shutdown_signal).await?;
```

`serve_with_shutdown` uses Axum's `with_graceful_shutdown` to stop accepting new connections after a shutdown signal is received and wait for in-flight requests to complete.
