# Admin Token Bootstrap (Controller and Gateway)

Describes where the Controller's Admin API token comes from at startup and the options available to operators. Authentication is **token-only** (2026-06-12): a single Istio-style pre-shared bearer token. The former browser-login stack (username/password `local_auth` + OIDC `auth` admin) was removed.

> The federation Center has been extracted to the separate EdgionCenter repository; its auth bootstrap is documented there, not here.

## Behavior Overview

| Component | No token configured | With a token configured |
|-----------|---------------------|-------------------------|
| Controller | **Server starts + business routes always 503** + WARN log at startup | Admin API accepts the pre-shared bearer token (allow-all) |
| Gateway | **Server starts + Admin API routes return 503** + WARN log at startup | Admin API accepts the configured shared bearer token |

The operator must supply the token explicitly — there is **no auto-generation**.

### 503 Semantics (fail-close)

When no token is configured (neither `EDGION_ADMIN_TOKEN` / `admin_token` nor any `cli_tokens`):

- `require_token_configured_gate` (admin app only) returns `503 Service Unavailable` on every business route.
- A startup WARN is logged: `no admin/cli token configured; Admin API business routes fail closed with 503 until EDGION_ADMIN_TOKEN (or a cli_token) is set`.
- `/health`, `/ready`, `/metrics`, and `/api/v1/auth/status` remain accessible (the static public set). `/api/v1/auth/status` reports `status: Initializing`.
- The controller never panics or exits.

## Controller: Providing the Admin Token

The Controller resolves the admin token from two sources (highest precedence first); a blank/whitespace value is treated as **not configured**.

### Option A: Env var from a K8s Secret (recommended for Kubernetes)

Inject `EDGION_ADMIN_TOKEN` via `valueFrom.secretKeyRef` from the `edgion-admin-credentials` Secret (key `token`) in the controller Deployment.

`secretKeyRef` is resolved by Kubernetes when the Pod is created; it does not require
the Controller process or its ServiceAccount to call the Secret API. The Controller's
separate resource-processing duties may still require Secret `get/list/watch` RBAC.

```bash
# Read the current admin token
kubectl -n edgion-system get secret edgion-admin-credentials \
    -o jsonpath='{.data.token}' | base64 -d
```

### Option B: `admin_token` in a Secret-mounted config file

Store the controller YAML config in a K8s Secret (not a ConfigMap — it holds a secret token) and mount it into the pod. The `admin_token` field is used directly when `EDGION_ADMIN_TOKEN` is absent.

```yaml
admin_token: "edg_…"
```

> **Warning**: do not store `admin_token` in a ConfigMap. Use the env var (Option A) or a Secret-mounted config file.

### Behavior when nothing is configured

If neither source provides a non-blank token (and no `cli_tokens` are configured either), the controller starts successfully but all Admin API business routes return `503 Service Unavailable` (fail-close) with a startup WARN. `/health`, `/ready`, `/metrics`, and `/api/v1/auth/status` remain accessible.

## Token model

- The admin token maps to an **allow-all wildcard** (`*`/`*`) RBAC policy — it is a `Role::Restricted` carrying that policy, held on the `Authorizer` via `set_admin_token`. There is no separate "superuser" role.
- `Authorizer::apply` merges the admin-token entry into the cli-token map at startup. Token configuration changes require a process restart.
- **Scoped, role-limited** tokens for automation are configured separately under `[cli_tokens]` (each with a fine-grained `rbac.allow` policy). See `skills/02-features/02-config/00-controller-config.md`.

## Gateway: Providing the Admin Token

The Gateway uses a separate shared bearer token for its own Admin API. Resolution is:

1. `EDGION_GATEWAY_ADMIN_TOKEN` when non-blank (recommended: inject it with a
   Kubernetes `secretKeyRef`).
2. `gateway.admin_token` in a Secret-mounted Gateway config file.

There is no CLI flag, which avoids placing the token in process arguments. A blank value
is treated as unconfigured. Without a token the Gateway still starts, but Admin API
business routes fail closed with `503`; probe endpoints remain on the separate probe
listener. The Gateway token is not the Controller token and does not use Controller
`cli_tokens` RBAC.

```yaml
gateway:
  admin_token: "edg_..."
```

Do not store this field in a ConfigMap. Prefer the environment variable from a Secret.

## Admin API network exposure

The Admin API is plaintext HTTP (no TLS listener). Its security relies on
network controls plus the bearer token. Secure defaults:

- `admin_listen` defaults to `127.0.0.1:12101` (loopback). Cluster/remote access
  requires explicitly setting `0.0.0.0` AND restricting reach via `allow_admin_ips`
  and/or a Kubernetes NetworkPolicy. K8s sample configs set `0.0.0.0` explicitly
  because the ClusterIP Service forwards to the Pod IP.
- `allow_admin_ips` is empty by default (allow all). Set it to the trusted
  peer CIDRs; matched against the TCP peer IP only.

When the controller starts bound to a non-loopback address with an empty
`allow_admin_ips`, it logs a high-severity WARN and sets the gauge:

    edgion_system_error_state{component="admin_api", reason="insecure_admin_exposure"} = 1

Example Alertmanager rule:

    edgion_system_error_state{reason="insecure_admin_exposure"} > 0 for 5m

The detection is observability-only — it never blocks startup. A hardening
NetworkPolicy template ships at `cicd/deploy/kubernetes/controller/optional/networkpolicy.yaml`.

## Token Revocation / Rotation

The pre-shared token is stateless — there is no session, cookie, or server-side blacklist. To revoke access:

1. **Rotate the admin token**: update the `edgion-admin-credentials` Secret (key `token`) and perform a rolling restart (`kubectl rollout restart deployment/edgion-controller -n edgion-system`). The old token stops working immediately on the new pods.
2. **Revoke a scoped cli token**: remove (or change) its `cli_tokens[].token_sha256` entry and restart every Controller replica. Old instances retain their startup token tables until they exit.

Rotating the admin token is the fastest bulk revocation; routine per-client scoping should use `[cli_tokens]` instead of sharing the admin token.

## CLI workflow

`edgion-cli` is token-only. It resolves the bearer token by precedence `--token` → `EDGION_TOKEN` → `~/.edgion/token` (`resolve_token`), with no auto-login or `/api/v1/auth/me` probe. `edgion-cli login --token <edg_…>` simply persists the token to `~/.edgion/token`. See `skills/01-architecture/04-ctl/01-commands.md`.

## Migration (BREAKING) — token-only auth

Deployments that relied on the old username/password or OIDC admin login will receive Admin-API 503 after upgrading until a token is provided. Migration:

1. **Mint a token** with `edgion-cli gen-token` (or any sufficiently random string), store it in the `edgion-admin-credentials` Secret under key `token`, and wire it into the controller Deployment as `EDGION_ADMIN_TOKEN` via `secretKeyRef`.
2. **Or** set `admin_token` in a Secret-mounted config file.
3. Update clients: replace any username/password or cookie flow with a `Authorization: Bearer <token>` header (or `edgion-cli login --token`).

Removed config keys: `local_auth` (username/password/jwt_secret/cookie_secure), `auth` (OIDC admin), and the `EDGION_ADMIN_USERNAME` / `EDGION_ADMIN_PASSWORD` / `EDGION_ADMIN_JWT_SECRET` env vars. The Admin API endpoints `/api/v1/auth/login`, `/api/v1/auth/logout`, and `/api/v1/auth/me` are gone; only the public `GET /api/v1/auth/status` remains.

> For data-plane OIDC/JWT on the **gateway** (the `openid_connect` / `jwt_auth` plugins), nothing changed — those are unrelated to Admin API auth.

## Related Regression Tests

- `edgion-tests/integration/scripts/integration/test_controller_no_auth.sh` —
  Integration-layer structural regression: no token configured → `/health`+`/ready` 200, business route 503, startup WARN; a second positive case exports `EDGION_ADMIN_TOKEN` and asserts an authenticated request succeeds.

## History

- 2026-06-12: Admin API auth switched to **token-only** (BREAKING). Removed the browser-login stack
  (`local_auth`, `unified_auth`, OIDC admin `auth`, the `/api/v1/auth/{login,logout,me}` endpoints,
  `Role::Superuser`). The operator now supplies a single Istio-style pre-shared bearer token via
  `EDGION_ADMIN_TOKEN` / `admin_token`; it maps to an allow-all wildcard policy and is loaded at startup.
  No token configured → server boots, business routes 503, startup WARN.
- 2026-06-01: Controller removed admin-credential auto-generation (BREAKING) — superseded by the
  token-only switch above.
