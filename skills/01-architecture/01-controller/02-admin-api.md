---
name: controller-admin-api
description: edgion-controller Admin HTTP API (Axum :5800) — full endpoint list, AdminState, health check, CRUD, and ConfigServer endpoints.
---

# Admin API (:5800)

The Controller exposes an HTTP Admin API via Axum, listening on port 5800, used by the edgion-cli command-line tool and operations workflows.

## Router Composition (compose_admin_routes)

Controller and Center share a single admin-router composition entry: `src/core/common/api/compose.rs::compose_admin_routes(business, auth_state, local_auth_intent)`.

The internal composition order is fixed (callers cannot influence it):

1. `business` (per-component business routes)
2. `/api/v1/auth/{login, logout, me}` (mounted when `local_auth_intent = true`)
3. `/api/v1/auth/status` (always mounted — fix #01 round 3 regression point; CLI `ensure_auth` requires it to be reachable at all times)
4. `unified_auth` middleware (wraps every route; `skip_paths` is provided by `auth_state`)

**Final-state contract**: the returned `Router` is final. Callers must not call `.route()` / `.layer()` on it again — any addition would bypass the auth middleware and is equivalent to the P0 bug from fix #01 round 2. New admin routes should be added to the `business` parameter before calling `compose_admin_routes`.

**CORS note**: the Admin API does not include a CORS layer. For local debugging, `edgion-dashboard`'s Vite `server.proxy` handles it (see `edgion-dashboard/vite.config.ts`); in production, the dashboard is embedded into the Controller and served from the same origin. If a separate-origin deployment is required in the future, introduce a precise origin allowlist (corresponds to the H1 fix record).

The three helpers (`add_local_auth_routes`, `add_auth_status_route`, `wrap_with_unified_auth_layer`) have been demoted to `pub(crate)`; direct external invocation is no longer supported. Use `compose_admin_routes` exclusively.

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
- `require_ready()` — `Ok(())` if ready, `Err(503)` otherwise; used by the write-path readiness middleware (see "Write-path readiness gate" below)
- `is_k8s_mode()` — whether the mode is Kubernetes (in K8s mode local schema validation is skipped)

## Full Endpoint List

### Health and Status

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Liveness probe. Returns `200` as soon as the service starts; response body `{"success":true,"data":"OK"}` |
| GET | `/ready` | Readiness probe. Returns `200` once ConfigSyncServer is ready, otherwise `503` |
| GET | `/api/v1/server-info` | Get the current `server_id` and ready state. After reload `server_id` changes, which can be used to confirm the reload took effect |

### Reload

| Method | Path | Description |
|--------|------|-------------|
| POST | `/api/v1/reload` | Trigger a full reload. Runs asynchronously: stop current controller -> clear PROCESSOR_REGISTRY -> create new ConfigSyncServer (new server_id) -> restart controller (full Init -> InitApply -> InitDone flow). Gateway clients re-list automatically once they detect the server_id change |

### Namespaced Resource CRUD

All endpoints read/write the **ConfCenter storage layer**. The request body supports JSON and YAML (JSON parsing is tried first).

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

Successful `POST` creates respond with **`201 Created`** and a `Location` header pointing at the new resource. `PUT` updates continue to use `200 OK` with the `ApiResponse::success(...)` body. Successful `DELETE` returns **`204 No Content`** with an empty body. Failed creates still surface as the mapped HTTP error (see "Error Mapping" below) and carry no `Location` header.

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

Parallel read-only endpoints returning `ResourceKey` (`apiVersion` / `kind` / `metadata` only — no `spec` / `status`). The legacy `/api/v1/{cluster,namespaced}/...` paths above are unchanged; the two families coexist by design.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/keys/cluster/{kind}` | List cluster-scoped resource keys |
| GET | `/api/v1/keys/namespaced/{kind}` | List keys across all namespaces |
| GET | `/api/v1/keys/namespaced/{kind}/{namespace}` | List keys in one namespace |

Query params: `limit=N` (page size) and `continue=<token>` (K8s-style cursor). Response carries `continue_token` for the next page (`null` on the last page).

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

| Probe | Endpoint | Semantics | K8s usage |
|-------|----------|-----------|-----------|
| Liveness | `/health` | Returns 200 as long as the process is alive | `livenessProbe` — failing causes the Pod to restart |
| Readiness | `/ready` | Returns 200 after ConfigSyncServer initialization is complete and all Processors are ready | `readinessProbe` — when not ready, the pod is removed from the Service endpoints |

Readiness is determined via `AdminState::is_ready()` -> `ConfMgr::is_ready()` -> `ConfCenter::is_ready()`, ultimately checking whether ConfigSyncServer has been created.

## Write-path readiness gate

The router applies `require_ready_middleware` (in `src/core/controller/api/mod.rs`) as a `route_layer` over the resource-CRUD write endpoints:

- `POST /api/v1/cluster/{kind}` / `PUT /api/v1/cluster/{kind}/{name}` / `DELETE /api/v1/cluster/{kind}/{name}`
- `POST /api/v1/namespaced/{kind}/{namespace}` / `PUT /api/v1/namespaced/{kind}/{namespace}/{name}` / `DELETE /api/v1/namespaced/{kind}/{namespace}/{name}`

While the controller is in init or reload (`is_ready() == false`), these endpoints respond with `503 Service Unavailable` instead of entering `parse_resource_and_update_version` and `ConfCenter::create_one / update_one / delete_one`. Without this gate, writes accepted during the LIST + cache-build window bypass cache consistency and (in non-K8s mode) race `update_resource_version` against the init flow's `resourceVersion` sequencing.

Endpoints intentionally not gated:

- `GET` endpoints on `/api/v1/...` (reads may return partial data during init — explicit operator/debug choice).
- `/health`, `/ready`, `/metrics`, `/api/v1/auth/*` — probes and auth must stay reachable.
- `/api/v1/reload` — `request_reload` itself returns an error when the center is not started, and reload is the mechanism for recovering from a stuck init.
- `/api/v1/services/acme/.../trigger` — notify-only, does not touch the storage layer.
- `/api/v1/diagnostics/...` and `/api/v1/server-info` — read-only diagnostic surface.
- `/configserver/...` — read-only cache surface; already returns 503 via `config_sync_server()` when not ready.

**Known gaps** (write paths that touch `ConfCenter` but are not yet covered by this gate, tracked separately):

- `POST /api/v1/cluster-region-routes/failover` and `POST /api/v1/service-region-routes/failover` — `region_route_handlers::patch_plugin_metadata_failover` calls `ConfCenter::update_one` on `PluginMetaData` but does not flow through `parse_resource_and_update_version`. Shares the same init-race risk; out of scope for the initial fix.
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
| `InternalError` | 500 Internal Server Error |
| `NotSupported` | 501 Not Implemented |
| `StalePagination` | 410 Gone |

## Authentication System

The Admin API integrates a unified authentication middleware that supports OIDC and local JWT modes.

> This section covers **authentication** (who you are) only. For **authorization** (the `(verb, kind)` RBAC engine, the `Role` model, cli tokens, and the full middleware onion), see [11-authentication-authorization.md](11-authentication-authorization.md). Authentication is mandatory — no-auth mode was removed (2026-05-24).

### Authentication Middleware (unified_auth)

Module location: `src/core/common/unified_auth/mod.rs`

Request authentication order:

1. Match against `skip_paths` (defaults: `/health`, `/ready`, `/metrics`) → pass through directly
2. Public auth endpoints (`/api/v1/auth/*`) → pass through directly
3. Try OIDC Bearer token validation (requires `[auth]` to be configured)
4. Try local JWT (HS256) Bearer token validation
5. Try reading the token from the httpOnly Cookie → validate as above
6. All fail → return `401 Unauthorized`

### Authentication Endpoints

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| POST | `/api/v1/auth/login` | not required | Username/password login; the response body contains the token, and an httpOnly Cookie is also set |
| POST | `/api/v1/auth/logout` | not required | Clear the httpOnly Cookie |
| GET | `/api/v1/auth/status` | not required (public) | Return the current auth mode (local/oidc/disabled) |
| GET | `/api/v1/auth/me` | required | Return the currently logged-in user info |

### httpOnly Cookie

After a successful login, the response sets an httpOnly JWT Cookie. Browsers automatically attach it on subsequent requests, so the front end does not need to manage tokens manually. This is suitable for the Dashboard front end.

### Admin Credential Bootstrap (Controller)

Admin credential auto-generation has been removed (as of 2026-06-01). The Controller now
behaves identically to the Center: when no valid credentials are configured, the server
starts but all Admin API business routes return 503 fail-close and a startup WARN is emitted.

Credentials are read from the following sources (highest precedence first):

1. **Env vars** `EDGION_ADMIN_USERNAME` / `EDGION_ADMIN_PASSWORD` / `EDGION_ADMIN_JWT_SECRET`
   injected via `valueFrom.secretKeyRef` from the `edgion-admin-credentials` Secret
   (recommended for Kubernetes). The `deploy.sh` script creates this Secret idempotently.
2. **`[local_auth]`** section in a Secret-mounted config file.
3. **`[auth]`** OIDC.

When nothing is configured: server starts, business routes return 503, `/health` / `/ready` /
`/metrics` / `/api/v1/auth/status` remain accessible.

The `secret_retry_loop` / `ensure_admin_credentials` / `process::exit(1)` machinery is gone.
The Controller ServiceAccount no longer needs the `secrets: create` RBAC verb.

See `skills/02-features/02-config/03-auth-bootstrap.md` for the full matrix and migration guide.

### Module Locations

| Module | Path |
|--------|------|
| Local auth (config/handlers/middleware/secret) | `src/core/common/local_auth/` |
| Unified auth middleware (OIDC + local) | `src/core/common/unified_auth/mod.rs` |

## Startup Modes

The Admin API supports two startup modes:

```rust
// regular startup
api::serve(conf_mgr, schema_validator, addr).await?;

// startup with graceful shutdown
api::serve_with_shutdown(conf_mgr, schema_validator, addr, shutdown_signal).await?;
```

`serve_with_shutdown` uses Axum's `with_graceful_shutdown` to stop accepting new connections after a shutdown signal is received and wait for in-flight requests to complete.
