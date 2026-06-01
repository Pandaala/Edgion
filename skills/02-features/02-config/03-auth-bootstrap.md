# Admin Credential Bootstrap (Auth Bootstrap)

Describes where the Controller / Center admin credentials come from at startup and the configuration paths available to operators.

## Behavior Overview

| Component | No `[local_auth]` + No `[auth]` | With `[local_auth]` | With `[auth]` OIDC |
|-----------|---|---|---|
| Controller | **Server starts + business routes always 503** + WARN log at startup | Static credentials from env vars or the configuration file | OIDC flow |
| Center | **Server starts + business routes always 503** + WARN log at startup | Static credentials from the configuration file | OIDC flow |

**Both components behave identically**: the operator must supply credentials explicitly.
Auto-generation has been removed from the Controller (as of 2026-06-01).

### Center's 503 Semantics (fail-close)

Center has no auto-generation capability. When neither `[local_auth]` nor `[auth]` is configured:
- `require_auth` derives to `true` (safe default: avoids leaving the admin API exposed).
- No auth provider is ready.
- `unified_auth_middleware` takes the "fail-close" branch and returns `503 Service Unavailable` (with `Retry-After: 10`).
- `/health`, `/ready`, `/metrics`, `/api/v1/auth/status` remain accessible (skip_paths + early-return paths).
- The startup log prints WARN: `No authentication configured. Admin endpoints will return 503 until you configure [local_auth] or [auth] in your YAML.`

## Controller: Providing Admin Credentials

The Controller reads admin credentials from the following sources (highest precedence first):

### Option A: Env vars from a K8s Secret (recommended for Kubernetes)

Inject `EDGION_ADMIN_USERNAME`, `EDGION_ADMIN_PASSWORD`, and `EDGION_ADMIN_JWT_SECRET` via
`valueFrom.secretKeyRef` in the controller Deployment. The `deploy.sh` script creates the
`edgion-admin-credentials` Secret idempotently before the controller is applied.

**RBAC prerequisite**: the Controller ServiceAccount needs only
`secrets: [get, list, watch]` — the `create` verb is no longer required. Both
`deploy/kubernetes/controller/rbac.yaml` and
`examples/k8stest/kubernetes/controller/rbac.yaml` reflect this.

```bash
# Read the current password
kubectl -n edgion-system get secret edgion-admin-credentials \
    -o jsonpath='{.data.password}' | base64 -d
```

### Option B: `[local_auth]` in a Secret-mounted config file

Store the controller YAML config in a K8s Secret (not a ConfigMap — it holds a password)
and mount it into the pod. The `[local_auth]` section in that file is used directly.

```yaml
local_auth:
  enabled: true
  username: admin
  password: "<your-password>"
  jwt_secret: "<random string, at least 32 characters>"
  jwt_expiry_hours: 24
```

> **Warning**: the controller config is currently mounted from a **ConfigMap** in the
> default manifests. A ConfigMap is not suitable for storing a password. Use env vars
> (Option A) or switch the mount to a Secret.

### Option C: `[auth]` OIDC

```yaml
auth:
  enabled: true
  discovery: "https://your-oidc-provider/.well-known/openid-configuration"
  client_id: "..."
```

### Behavior when nothing is configured

If none of the above sources provides valid credentials, the controller starts successfully
but all Admin API business routes return `503 Service Unavailable` (fail-close). A WARN is
printed at startup: `No authentication configured. Admin endpoints will return 503 until
you configure [local_auth] or [auth] in your YAML.` The controller never panics or exits.

`/health`, `/ready`, `/metrics`, and `/api/v1/auth/status` remain accessible.

## Center: How to Provide Admin Credentials

### Option A: `local_auth` local username/password (YAML)

```yaml
local_auth:
  enabled: true
  username: admin
  # bcrypt hash. Generate via: htpasswd -bnBC 10 "" <your-password> | tr -d ':\n'
  # Or use a plaintext password field (deploy/center-test/deploy.yaml has a test example)
  password: "<your-password>"
  jwt_secret: "<a sufficiently long random string, at least 32 characters>"
  jwt_expiry_hours: 24
  skip_paths:
    - "/health"
    - "/ready"
```

### Option B: `auth` OIDC

```yaml
auth:
  enabled: true
  discovery: "https://your-oidc-provider/.well-known/openid-configuration"
  client_id: "..."
  # ...
```

### Consequences of Configuring Nothing (fail-close via 503)

Center starts successfully, but:
- All `/api/v1/*` business paths are rejected with 503.
- `/health` / `/ready` / `/metrics` / `/api/v1/auth/status` remain accessible.
- The startup log prints WARN (see above).

The operator can supply the configuration and restart afterward.

### No-Auth Mode Removed (as of 2026-05-24)

No-auth mode — setting `local_auth.enabled: false` (or `auth.enabled: false`) with the intent of bypassing authentication — is **no longer supported**. Authentication is mandatory.

The `enabled: false` flag is now **ignored** with a startup `WARN` log (not a hard error, to avoid CrashLoops on upgrade). The behavior after that warning depends on the component:

| Scenario | Controller behavior | Center behavior |
|----------|--------------------|----|
| Config absent (no `local_auth` / `auth` section at all) | Business routes return 503 until configured; emits startup WARN | Business routes return 503 until configured |
| `local_auth.enabled: false` (explicit disable) | No provider is loaded → admin API returns 503 until reconfigured; emits startup WARN | Same: 503 fail-close until reconfigured; emits startup WARN |
| `local_auth.enabled: true` (or section absent) | Normal operation | Normal operation |

**Upgrade path**: if you previously relied on `enabled: false` to "disable local_auth while using OIDC":
- Remove the `local_auth` section entirely, or set `enabled: true` with valid credentials.
- Configure the `auth` section with `enabled: true` + a valid OIDC discovery URL.
- The `oidc_intent = true` path runs normally regardless of the `local_auth` field.

## K8s Deployment: Mounting Center YAML via Secret

Recommended pattern: store the Center YAML configuration (including the `local_auth:` section) as a K8s Secret and mount it into the Center Pod as the main configuration file.

Reference: `deploy/center-test/deploy.yaml`.

## JWT Revocation Semantics (`[local_auth]` mode)

**Current state**: local admin JWTs are **stateless** — Edgion has no server-side token blacklist; `POST /api/v1/auth/logout` only clears the browser cookie (`Max-Age=0`); **issued tokens remain valid until they reach `exp`**.

Upper bound on the exposure window = `jwt_expiry_hours` (default `24`).

### This Is an Intentional Stateless Trade-off

Same pattern as **Argo CD local administrator** / **Kubernetes Dashboard built-in token**. Introducing a server-side blacklist runs into two architectural difficulties:

| Approach | Problem |
|----------|---------|
| In-memory blacklist (`HashSet<jti>`) | In multi-instance HA deployments each instance is independent — logout hits instance A while subsequent requests hit instance B → B does not know about the revocation = **illusory defense**; process restarts wipe it. |
| Redis blacklist | Introduces a hard Redis dependency on the admin API path + 1 RT latency per request + a fail-open vs. fail-close dilemma when Redis is unavailable. |

So Edgion chooses "stateless JWT + provide revocation through other means".

### When Real Revocation Is Needed: **Use OIDC Federation**

`unified_auth` supports configuring OIDC in the `[auth]` section (Keycloak / Auth0 / Okta / Azure AD / Google, etc.):
- Revoke a user session or refresh token at the IdP → all access tokens based on that session become invalid.
- Unified session lifecycle management, revocation, and audit logs at the IdP.
- This is the **industry-standard path** when production-grade revocation is genuinely required for "management-side authentication".

Switch example (either Controller or Center):

```yaml
# Disable local_auth, switch to OIDC
local_auth:
  enabled: false
auth:
  enabled: true
  discovery: "https://idp.example.com/.well-known/openid-configuration"
  client_id: "edgion-admin"
  client_secret_ref:
    name: oidc-admin-secret
  # ...
```

### If You Must Use `[local_auth]`: Shorten the Exposure Window

For deployments that cannot yet use OIDC:

```yaml
local_auth:
  enabled: true
  # ...
  jwt_expiry_hours: 4       # Production recommendation: 1-4 hours rather than the default 24h
```

**About the CLI workflow**: `edgion-cli`'s `ensure_auth()` automatically **re-logs in** after the token expires or the controller restarts (provided that credentials can be parsed from the configuration file). Shortening `jwt_expiry_hours` does not break the CLI user experience; it only makes the browser dashboard scenario re-prompt for login more often.

### Standard Operations for Decommissioning (Alternatives to "Revocation")

Since per-token revocation is unavailable, when handling operator departures / privilege removal:

1. **Change the password**: update the `edgion-admin-credentials` Secret and perform a rolling restart (`kubectl rollout restart deployment/edgion-controller -n edgion-system`) → all future logins must use the new password (but **issued tokens** still remain valid until `exp`).
2. **Rotate `jwt_secret`**: modify the `jwt_secret` field in the Secret → all issued tokens become invalid immediately (**one-click revoke all**, but it also kicks off all currently logged-in administrators).
3. **Wait for the window to expire**: if `jwt_expiry_hours` has been shortened to 1-4h, this is also acceptable.

**Option 2 (rotate jwt_secret) is the fastest "bulk revocation"**, suitable for emergencies; routine personnel changes should be paired with OIDC federation.

## Migration (BREAKING) — Controller credential auto-generation removed

Existing deployments that relied on the Controller auto-generating the
`edgion-admin-credentials` Secret will receive Admin-API 503 after upgrading until
credentials are provided. Migration paths:

1. **Run the updated `deploy.sh`** (idempotent). If the Secret already exists from the old
   auto-generation, it is left untouched (current password preserved). The updated
   `controller/deployment.yaml` wires the Secret into the controller via env vars, so a
   `kubectl apply` of the new manifests is sufficient after `deploy.sh` completes.
2. **Or** set `EDGION_ADMIN_PASSWORD` and `EDGION_ADMIN_JWT_SECRET` from any Secret and
   patch the Deployment to use `secretKeyRef`.
3. **Or** write `[local_auth]` into a Secret-mounted config file (not a ConfigMap — a
   ConfigMap is not suitable for storing a password).

The existing auto-generated `edgion-admin-credentials` Secret already has the correct keys
(`username`, `password`, `jwt_secret`), so migration path 1 is transparent for the
operator.

## Related Regression Tests

- `examples/test/scripts/integration/test_center_no_auth.sh` —
  Integration-layer structural regression for Center starting without auth configured + 503 on `/api/v1/controllers` business paths + WARN log.
  Included in the prerequisites of `run_integration.sh`.
- `examples/test/scripts/integration/test_controller_no_auth.sh` (added 2026-06-01) —
  Mirrors the Center test for the Controller: no auth config → `/health`+`/ready` 200,
  business route 503, startup WARN; second positive case exports `EDGION_ADMIN_PASSWORD`
  + `EDGION_ADMIN_JWT_SECRET` and asserts login succeeds.

## History

- 2026-06-01: Controller removed admin-credential auto-generation (BREAKING).
  - The Controller now behaves identically to Center: no `[local_auth]` / no `[auth]` →
    server starts + business routes 503 + startup WARN. Auto-generation
    (`ensure_admin_credentials`, `secret_retry_loop`, the `secrets: create` RBAC verb) is
    deleted.
  - Credentials are supplied via env vars from a K8s Secret (`secretKeyRef`, recommended),
    `[local_auth]` in a Secret-mounted config file, or `[auth]` OIDC.
  - `deploy.sh` handles idempotent Secret creation for the operator.
  - Related spec: `docs/superpowers/specs/2026-06-01-remove-controller-admin-credential-autogen-design.md`.
- 2026-04-20: Center removed the default `admin/edgion1234` fallback (BREAKING).
  - Root cause: the bug 1 in fix #01 round 2 (`LocalAuthConfig::default()` panicked at startup) was rooted in
    the Center's `or_else(|| Some(edgion1234))` fallback constructing credentials for the operator that did not
    satisfy fix #01's strengthened validate.
  - Fix: Center no longer provides a fallback; the operator must explicitly configure `[local_auth]` or `[auth]`,
    otherwise the admin API returns 503 across the board.
  - Related spec: `docs/superpowers/specs/2026-04-20-admin-server-unification-design.md`.
