# Admin Credential Bootstrap (Auth Bootstrap)

Describes where the Controller / Center admin credentials come from at startup and the configuration paths available to operators.

## Behavior Overview

| Component | No `[local_auth]` + No `[auth]` | With `[local_auth]` | With `[auth]` OIDC |
|-----------|---|---|---|
| Controller | Auto-generates Secret `edgion-admin-credentials` (K8s mode) or writes to the conf directory (local mode); log in with the generated password | Static credentials from the configuration file | OIDC flow |
| Center | **Server starts + business routes always 503** + WARN log at startup | Static credentials from the configuration file | OIDC flow |

**Key difference**: Controller supports "zero-config startup" (Secret auto-generation is retained); Center requires the operator to provide credentials explicitly.

### Center's 503 Semantics (fail-close)

Center has no auto-generation capability. When neither `[local_auth]` nor `[auth]` is configured:
- `require_auth` derives to `true` (safe default: avoids leaving the admin API exposed).
- No auth provider is ready.
- `unified_auth_middleware` takes the "fail-close" branch and returns `503 Service Unavailable` (with `Retry-After: 10`).
- `/health`, `/ready`, `/metrics`, `/api/v1/auth/status` remain accessible (skip_paths + early-return paths).
- The startup log prints WARN: `No authentication configured. Admin endpoints will return 503 until you configure [local_auth] or [auth] in your TOML.`

## Controller: Reading the Auto-generated Credentials

### K8s Mode

```bash
kubectl -n edgion-system get secret edgion-admin-credentials \
    -o jsonpath='{.data.password}' | base64 -d
```

### Local Mode

```bash
cat {conf_dir}/Secret_edgion-system_edgion-admin-credentials.yaml
```

### Changing the Password

After `kubectl edit secret edgion-admin-credentials`, the Controller's secret retry loop hot-reloads
`UnifiedAuthState` (semantics are stable under the CancellationToken shutdown signal; see the structural regression in fix #01 round 3).

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

### Consequences of Explicitly Disabling All Providers (no-auth mode, **dangerous**)

If the operator **explicitly** sets all providers to `enabled: false`, for example:

```yaml
local_auth:
  enabled: false
auth:
  enabled: false
```

Then:
- `require_auth = false` (respecting the operator's opt-out intent).
- `unified_auth_middleware` directly passes through all requests.
- **All `/api/v1/*` business paths are accessible without a token** — the admin API is **completely open**.
- Startup WARN: `Admin API is running in NO-AUTH mode: ...` (since 2026-04-20).

This is a legitimate "pure no-auth mode" (e.g., internal-network isolation + authentication handled by another gateway layer), but it is **very easy to trigger by mistake** (e.g., the operator wants to "temporarily disable local_auth and use OIDC only" but accidentally disables both). If your deployment really takes this path:

1. Explicitly state in the deployment documentation that this is "no-auth mode, relying on layer X for authentication".
2. Do not place this YAML on a Center exposed to the public internet.

If you simply want to **switch** to OIDC-only, the correct approach is:
- **Remove** the `local_auth` section entirely (or leave only `enabled: false`).
- Configure the `auth` section with `enabled: true` + a valid discovery.
- Then `oidc_intent = true`, `require_auth = true`, and the normal OIDC flow runs.

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

1. **Change the password**: `kubectl edit secret edgion-admin-credentials` to modify the `password` field → the Controller secret retry loop hot-reloads `UnifiedAuthState` → all future logins must use the new password (but **issued tokens** still remain valid until `exp`).
2. **Rotate `jwt_secret`**: modify the `jwt_secret` field in the Secret → all issued tokens become invalid immediately (**one-click revoke all**, but it also kicks off all currently logged-in administrators).
3. **Wait for the window to expire**: if `jwt_expiry_hours` has been shortened to 1-4h, this is also acceptable.

**Option 2 (rotate jwt_secret) is the fastest "bulk revocation"**, suitable for emergencies; routine personnel changes should be paired with OIDC federation.

## Related Regression Tests

- `examples/test/scripts/integration/test_center_no_auth.sh` —
  Integration-layer structural regression for Center starting without auth configured + 503 on `/api/v1/controllers` business paths + WARN log.
  Included in the prerequisites of `run_integration.sh`.

## History

- 2026-04-20: Center removed the default `admin/edgion1234` fallback (BREAKING).
  - Root cause: the bug 1 in fix #01 round 2 (`LocalAuthConfig::default()` panicked at startup) was rooted in
    the Center's `or_else(|| Some(edgion1234))` fallback constructing credentials for the operator that did not
    satisfy fix #01's strengthened validate.
  - Fix: Center no longer provides a fallback; the operator must explicitly configure `[local_auth]` or `[auth]`,
    otherwise the admin API returns 503 across the board.
  - Related spec: `docs/superpowers/specs/2026-04-20-admin-server-unification-design.md`.
