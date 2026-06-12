---
name: controller-auth-authz
description: edgion-controller authentication + authorization architecture — the three request entry paths (unified_auth, cli token, federation mTLS), the authz engine (Role/Verb/CenterPolicy), the Admin API middleware onion, the built-in default Center policy, and the CLI token workflow.
---

# 11 Authentication & Authorization (Controller)

> **Authentication** answers *who are you* (unified_auth session, cli token, or federation mTLS).
> **Authorization** answers *what may you do* — a default-deny `(verb, kind)` RBAC engine.
> Both are **mandatory**: no-auth mode was removed (2026-05-24). An unauthenticated request gets `401`; an unauthorized one gets `403`.

This file is the architecture entry point for the Controller's auth stack. Configuration schemas live in
[`../../02-features/02-config/00-controller-config.md`](../../02-features/02-config/00-controller-config.md)
(the `[local_auth]`, `[auth]`, `[center]`, and `[cli_tokens]` sections); credential bootstrap lives in
[`../../02-features/02-config/03-auth-bootstrap.md`](../../02-features/02-config/03-auth-bootstrap.md).

## At a glance — three request entry paths

| Entry path | Authentication | Role assigned | Authorization | Default access |
|------------|----------------|---------------|---------------|----------------|
| Browser / admin login | `unified_auth` (OIDC or local JWT, Bearer or httpOnly cookie) | `Superuser` | `authz_layer` (Superuser allows everything) | allow-all |
| edgion-cli automation | `cli_token_pre_authn` (Bearer, SHA-256 lookup) | `Restricted(policy)` | `authz_layer` against the token's policy | **deny-all** until granted |
| Federation Center | mTLS peer identity (outbound gRPC channel, no listener) | `Restricted(center_policy)` | `authz_layer` against `center.rbac` | built-in default policy (see below) |

All three converge on the **same** `authz_layer` and the same `Authorizer`. The only difference is which `Role` lands in the request extensions.

## Authentication (who are you)

### unified_auth — the shared session gatekeeper

The shared middleware (`edgion-common/src/unified_auth/mod.rs`) validates OIDC Bearer tokens, local HS256 JWTs, and the httpOnly cookie, in that order. It wraps *every* admin route via `compose_admin_routes`. Full request order, endpoints, cookie behavior, and auto-generated credentials are documented in
[`02-admin-api.md` → Authentication System](02-admin-api.md#authentication-system).

Authentication is mandatory. `require_auth` always derives to `true`; the old `!require_auth` pass-through early-return was deleted. Setting `enabled: false` is ignored with a startup `WARN` (it no longer disables auth). See
[`03-auth-bootstrap.md` → No-Auth Mode Removed](../../02-features/02-config/03-auth-bootstrap.md#no-auth-mode-removed-as-of-2026-05-24).

### cli_token_pre_authn — static tokens for automation

Module: `edgion-controller/src/api/cli_token_middleware.rs`.

A Controller-private pre-authn layer that runs **outside** `unified_auth`, letting edgion-cli authenticate with a static Bearer token instead of an interactive login. Key properties:

- **Bearer-only**: reads only the `Authorization: Bearer <token>` header (case-insensitive). Cookie callers are browser admins and fall through to `unified_auth`.
- **SHA-256 stored**: the lookup hashes the incoming token (`sha256_hex`) and compares against `cli_tokens[].token_sha256`. Raw tokens are never held in config or memory.
- **Hot-reloadable**: the token table lives in an `ArcSwap` and is refreshed on `/api/v1/reload` without restart.
- **Additive**: a recognized token injects `Role::Restricted` + `AuthBypass` + `CliTokenIdentity`. An unrecognized/absent token is left untouched and falls through to `unified_auth` (which then 401s if no session).
- **Auth-route skip**: `/api/v1/auth/*` is never processed here — injecting `AuthBypass` there would make `unified_auth` skip while `me_handler` still needs `Extension<UnifiedAuthClaims>`, producing a 500.

The `AuthBypass` marker (`edgion-common/src/unified_auth/`) is the decoupling seam: cli-pre signals "already authenticated" so `unified_auth` skips its own checks, **without** `unified_auth` ever importing the authz engine (a maintained invariant — `common/unified_auth` is authz-free).

### Federation mTLS — the Center identity

Federation requests are served by a **separate admin router instance** (`fed_router`), built from `create_admin_router` wrapped with only `authz_layer` + `inject_center_identity` — it has **no `unified_auth` and no cli-pre layer**, so the Center path and the local-admin path are fully isolated router stacks. `inject_center_identity` snapshots the current Center policy as `Role::Restricted`. (The Controller↔Center link itself is an outbound gRPC channel from the Controller, not an inbound listener.) Details:
[`../../04-review/architecture/fed-proxy-mtls-fail-close.md`](../../04-review/architecture/fed-proxy-mtls-fail-close.md).

## Authorization (what may you do)

### Engine location

The authz engine lives in `edgion-controller/src/authz/mod.rs` — **Controller-private**. It was relocated out of `common/authz` so the Center can never take a dependency on the Controller's policy/identity model. The shared config *data* types (`FedRbacConfig`, `FedAllowRule`) stay in `common/config` (they are referenced by `CenterClientConfig` there); only the engine is Controller-private.

> **Center scoping**: Center uses the shared `unified_auth` for authentication (mandatory; no auto-generated credentials; 503 fail-close when unconfigured) but has **no authz layer** — there is no `(verb, kind)` RBAC engine, no `Role`, no `CenterPolicy` on the Center side. The Center has been extracted to the separate EdgionCenter repository; see that repository for Center auth details.

### Role model

```rust
pub enum Role {
    Superuser,                      // authenticated local/OIDC admin: allow-all
    Restricted(Arc<CenterPolicy>),  // federation Center identity AND cli tokens
}
```

`Restricted` is shared by both the Center and cli tokens — they differ only in *which* `CenterPolicy` they carry. `Authorizer::authorize(role, verb, kind)`: `Superuser → Allow`; `Restricted(p) → p.permits(verb, kind)`.

### Verbs & request classification

`authz_layer` classifies each request into a `(Verb, kind)` pair via the axum `MatchedPath` route template + the `{kind}` path param (extracted with `RawPathParams`, so authz sees the same percent-decoded value the handler does). The closed `Verb` set (safe as a metric label):

```
get  list  list-keys  watch  create  update  delete
failover  acme-trigger  reload  server-info  diagnostics  wipe-all  debug
```

`kind` is usually a `ResourceKind` string (e.g. `HTTPRoute`), but some endpoints map to **synthetic** classifier labels that are not ResourceKinds — e.g. `RegionRoute` for the aggregate region-route / failover endpoints. An unknown/unclassifiable route is **denied** (`403`), never allowed through.

### CenterPolicy semantics

`CenterPolicy` is **default-deny**: an empty policy authorizes nothing. A request is permitted when *any* allow rule matches both the verb and the kind:

```
permits(verb, kind) = any rule where
    (rule.verbs contains "*" OR contains verb)   // verb compared lowercase
  AND (rule.kinds contains "*" OR contains kind)  // kind compared case-SENSITIVE
```

- **Verb strings are matched lowercase** (config verbs are lowercased at compile time).
- **Policy config kinds are case-sensitive** — a `cli_tokens` / `center.rbac` rule must name the canonical PascalCase kind exactly (`HTTPRoute`). Incoming request paths are normalized to the canonical kind by the classifier *before* the policy check, so URL casing itself is not significant — only the casing of the kind strings you write in config.
- `verbs: ["*"]` over the full admin router is effectively Superuser — grant narrowly.

### Built-in default Center policy

Module: `edgion-controller/src/fed_sync/default_policy.rs`. Applied **only when `center.rbac` is absent** (if the operator supplies `center.rbac`, it is used verbatim and the default is not applied). The default is a minimal, useful-out-of-the-box allow set:

- **Read** (`get` / `list` / `watch` / `list-keys`) on **every ResourceKind except `Secret`**. `Secret` is excluded so the default policy never exposes credentials to federation clients. (`EdgionTls` is *not* excluded — it is a link reference, not raw key material.)
- **Write** (`create` / `update` / `delete`) on **`PluginMetaData` only** (the region-route and ip-restriction handlers drive this kind).
- **`get` / `list` / `failover`** on the synthetic **`RegionRoute`** label (aggregate read endpoints + the POST failover endpoint).
- **Nothing else**: no `reload`, `server-info`, `diagnostics`, `wipe-all`, `debug`.

A compile-time exhaustiveness gate (`is_default_readable`) forces every new `ResourceKind` to make an explicit readable/excluded decision — adding a kind without updating it is a compile error.

## Middleware onion (local admin server)

`serve_with_shutdown` assembles the layers so the runtime order (outermost first) is:

```text
cli_auth_status_augment        (outermost — rewrites the /api/v1/auth/status body only)
  cli_token_pre_authn          (injects Role + AuthBypass + CliTokenIdentity for known cli tokens)
    cache_control_no_store     (blanket Cache-Control: no-store; added inside compose_admin_routes)
      unified_auth             (skips when AuthBypass present)
        assign_superuser_if_no_role   (route_layer — added second)
          authz_layer          (route_layer — added first, so it is innermost)
            handler
```

The two outermost layers are cross-cutting and do **not** affect auth/authz semantics: `cli_auth_status_augment` only appends `"cli"` to the `/api/v1/auth/status` `active_providers` list when cli tokens are registered, and `cache_control_no_store` sets a blanket `Cache-Control: no-store`. The auth/authz sandwich is the inner four layers: `cli_token_pre_authn → unified_auth → assign_superuser_if_no_role → authz_layer`.

axum `route_layer`s apply in **LIFO** order (the later `.route_layer()` wraps the earlier one), so the code calls `.route_layer(authz_layer)` first and `.route_layer(assign_superuser_if_no_role)` second — making `assign_superuser_if_no_role` run *before* `authz_layer`. `route_layer` (not `.layer`) means these never run on the 404 fallback. The `/api/v1/auth/*` routes are mounted in `compose_admin_routes` *outside* the business router, so they are covered by `unified_auth` but not by the authz `route_layer`.

### assign_superuser_if_no_role — fail-closed role assignment

`edgion-controller/src/api/authz_middleware.rs`. Runs after cli-pre and after `unified_auth`:

1. **Role already present** (cli-pre injected `Restricted`) → pass through unchanged.
2. **`AuthBypass` present but no `Role`** → invariant violation → **fail closed: `403`** (recorded as a `record_rbac_denied(..., "unknown")` so the bug path is observable). Keyed on `AuthBypass` alone (not `AuthBypass && CliTokenIdentity`) so any future bypass source without a role can never silently escalate to Superuser.
3. **Authenticated admin via `unified_auth`, no role, no bypass** → insert `Role::Superuser`.

### authz_layer — the decision point

`authz_layer` reads the `Role` (missing role → `403`), classifies the request, and calls `Authorizer::authorize`. Allow → `next`; Deny → `403`. A skip set bypasses authz entirely (mirroring the authn skip set): `/health`, `/ready`, `/metrics`, `/api/v1/auth/{login,logout,status}`.

## CLI token workflow

1. **Mint**: `edgion-cli gen-token` — generates a CSPRNG token with an `edg_` prefix, prints both the token (for the client) and its SHA-256 (for the controller). The hash covers the **full `edg_`-prefixed string** (exactly what the Bearer header carries).
2. **Configure**: paste the SHA-256 into a `cli_tokens[]` entry with a `name` and an `rbac.allow` rule set. Schema + examples:
   [`../../02-features/02-config/00-controller-config.md` → `[cli_tokens]`](../../02-features/02-config/00-controller-config.md).
3. **Reload**: `POST /api/v1/reload` hot-swaps the token table — no restart.
4. **Use**: `edgion-cli --token <edg_…>` (long-only; `-t` is taken by `--target`) or the `EDGION_TOKEN` env var. Supplying a token short-circuits `ensure_auth` (no `/auth/status`, no auto-login) and does **not** touch `~/.edgion/token` (ephemeral per-invocation).

Operator walkthrough and the full security checklist live in the human-facing ops guide: `docs/en/ops-guide/federation-rbac.md#cli-token-rbac`.

## Observability

Denials increment `edgion_fed_rbac_denied_total` via `record_rbac_denied(verb, kind, source)` (`edgion-common/src/observe/fed_metrics.rs`). The `source` label is bounded to `center | cli_token | unknown` — the cli-token *name* is **never** a metric label (unbounded cardinality); it appears only in the structured deny log (`token_name` / `provider` fields). `source` is derived from the presence of `CliTokenIdentity` in the request extensions.

## Module map

| Concern | Path |
|---------|------|
| Authz engine (`Role` / `Verb` / `CenterPolicy` / `Authorizer` / `Decision`) | `edgion-controller/src/authz/mod.rs` |
| authz_layer + assign_superuser_if_no_role + inject_center_identity | `edgion-controller/src/api/authz_middleware.rs` |
| Request → `(verb, kind)` classifier | `edgion-controller/src/api/authz_classifier.rs` |
| cli_token_pre_authn + `CliTokenIdentity` | `edgion-controller/src/api/cli_token_middleware.rs` |
| Built-in default Center policy | `edgion-controller/src/fed_sync/default_policy.rs` |
| Shared session auth (OIDC + local JWT) + `AuthBypass` marker | `edgion-common/src/unified_auth/mod.rs` |
| Local username/password (config/handlers/secret) | `edgion-common/src/local_auth/` |
| Admin router composition (`compose_admin_routes`) | `edgion-common/src/api/compose.rs` |
| `cli_tokens` / `center.rbac` config types (`FedRbacConfig`) | `edgion-common/src/config/mod.rs` |

## Related docs

- [`02-admin-api.md`](02-admin-api.md) — Admin API endpoints + `unified_auth` request order.
- [`../../02-features/02-config/00-controller-config.md`](../../02-features/02-config/00-controller-config.md) — `[local_auth]` / `[auth]` / `[center]` / `[cli_tokens]` schemas.
- [`../../02-features/02-config/03-auth-bootstrap.md`](../../02-features/02-config/03-auth-bootstrap.md) — credential bootstrap, no-auth removal, JWT revocation.
- [`../../04-review/architecture/fed-proxy-mtls-fail-close.md`](../../04-review/architecture/fed-proxy-mtls-fail-close.md) — federation mTLS fail-close + Center identity injection.
