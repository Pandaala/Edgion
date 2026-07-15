---
name: controller-auth-authz
description: edgion-controller authentication + authorization architecture — token-only (Istio-style pre-shared bearer) admin authn, the two request entry paths (local admin token, federation mTLS), the authz engine (Role/Verb/CenterPolicy), the Admin API middleware onion, the built-in default Center policy, and the CLI token workflow.
---

# 11 Authentication & Authorization (Controller)

> **Authentication** answers *who are you* — bearer-token-only admin and scoped CLI credentials.
> **Authorization** answers *what may you do* — a default-deny `(verb, kind)` RBAC engine.
> Authentication is **token-only**: the browser-login / username-password / OIDC admin stack was removed. With tokens configured, an unrecognized or absent token gets `401`; a recognized token denied by policy gets `403`. With no token configured at all, business routes fail closed with `503`.

This file is the architecture entry point for the Controller's auth stack. Configuration schemas live in
[`../../02-features/02-config/00-controller-config.md`](../../02-features/02-config/00-controller-config.md)
(the `admin_token`, `[center]`, and `[cli_tokens]` sections); credential bootstrap lives in
[`../../02-features/02-config/03-auth-bootstrap.md`](../../02-features/02-config/03-auth-bootstrap.md).

## At a glance — two request entry paths

| Entry path | Authentication | Role assigned | Authorization | Default access |
|------------|----------------|---------------|---------------|----------------|
| Local admin (operator + edgion-cli) | `cli_token_pre_authn` (Bearer, SHA-256 lookup) | `Restricted(policy)` | `authz_layer` against the token's policy | admin token → allow-all wildcard; scoped cli tokens → **deny-all** until granted |
| Federation Center | mTLS peer identity (outbound gRPC channel, no listener) | `Restricted(center_policy)` | `authz_layer` against `center.rbac` | built-in default policy (see below) |

Both paths converge on the **same** `authz_layer` and the same `Authorizer`. The only difference is which `Role` (which `CenterPolicy`) lands in the request extensions. The local admin token is itself just a `Restricted` role carrying an allow-all wildcard (`*`/`*`) policy — there is no separate "superuser" code path.

## Authentication (who are you)

### Token-only admin authn — `cli_token_pre_authn`

Module: `edgion-controller/src/api/cli_token_middleware.rs`. This is the **sole** Admin API authentication layer.

- **Bearer-only**: reads only the `Authorization: Bearer <token>` header (case-insensitive). There is no cookie/session concept and no interactive login.
- **SHA-256 lookup**: the incoming token is hashed and compared against the `Authorizer` token map, which stores hashes. Scoped CLI config stores hashes; the raw admin token temporarily exists in env/config parsing memory before startup hashes and registers it.
- **One map, two sources**: both the local admin token (allow-all wildcard policy, reserved name `"admin"`) and the scoped Center `cli_tokens` live in the same map, so this one middleware authenticates both.
- **Additive, no bypass**: a recognized token injects `Role::Restricted` + `CliTokenIdentity`. An unrecognized/absent token injects nothing; `authz_layer` returns `401` with `WWW-Authenticate: Bearer` for the role-less request, except on static public paths.

The local admin token is registered onto the `Authorizer` via `set_admin_token` (built by `build_admin_token_entry`, which rejects a blank/whitespace token → not configured) and merged into the live cli-token map by `Authorizer::apply` — so it **survives every reload** even though reloads rebuild the cli-token map purely from `config.cli_tokens`.

### Federation mTLS — the Center identity

Federation requests are served by a **separate admin router instance** (`fed_router`), built from `create_admin_router` wrapped with only `authz_layer` + `inject_center_identity` — it has **no token-authn layer and no 503 token gate**, so the Center path and the local-admin path are fully isolated router stacks. `inject_center_identity` snapshots the current Center policy as `Role::Restricted`. (The Controller↔Center link itself is an outbound gRPC channel from the Controller, not an inbound listener.) Details:
[`../../04-review/architecture/fed-proxy-mtls-fail-close.md`](../../04-review/architecture/fed-proxy-mtls-fail-close.md).

## Authorization (what may you do)

### Engine location

The authz engine lives in `edgion-controller/src/authz/mod.rs` — **Controller-private**. Federation RBAC config data (`FedRbacConfig`, `FedAllowRule`) also lives in `edgion-controller/src/config/mod.rs`; `edgion-common` carries neither this engine nor its Controller policy model.

> **Center scoping**: The Center has been extracted to the separate EdgionCenter repository; see that repository for Center auth details. The Controller side has no Center-internal auth state.

### Role model

```rust
pub enum Role {
    Restricted(Arc<CenterPolicy>),  // EVERY identity: admin token, cli tokens, federation Center
}
```

There is a single role variant. `Restricted` is shared by the local admin token, the scoped cli tokens, and the Center — they differ only in *which* `CenterPolicy` they carry. The local admin token carries an allow-all wildcard policy (`admin_wildcard_policy`), so "admin = allow everything" falls out of normal policy evaluation rather than a special case. `Authorizer::authorize(role, verb, kind)`: `Restricted(p) → p.permits(verb, kind)`.

### Verbs & request classification

`authz_layer` classifies each request into a `(Verb, kind)` pair via the axum `MatchedPath` route template + the `{kind}` path param (extracted with `RawPathParams`, so authz sees the same percent-decoded value the handler does). The closed `Verb` set (safe as a metric label):

```
get  list  list-keys  watch  create  update  delete
failover  acme-trigger  conf-sync-rotate  reload  server-info  diagnostics  wipe-all  debug
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
- `verbs: ["*"]` + `kinds: ["*"]` is the allow-all wildcard — exactly what the local admin token gets. Grant scoped cli tokens narrowly.

### Built-in default Center policy

Module: `edgion-controller/src/fed_sync/default_policy.rs`. Applied **only when `center.rbac` is absent** (if the operator supplies `center.rbac`, it is used verbatim and the default is not applied). The default is a minimal, useful-out-of-the-box allow set:

- **Read** (`get` / `list` / `watch` / `list-keys`) on **every ResourceKind except `Secret`**. `Secret` is excluded so the default policy never exposes credentials to federation clients. (`EdgionTls` is *not* excluded — it is a link reference, not raw key material.)
- **Write** (`create` / `update` / `delete`) on **`EdgionConfigData` only** (the region-route and ip-restriction handlers drive this kind).
- **`list` / `failover`** on the synthetic **`RegionRoute`** label. `get` is intentionally absent because no classifier route emits `(get, RegionRoute)`.
- **`server-info`** on synthetic **`NonResource`** for read-only server identity/readiness checks.
- **Nothing else**: no `reload`, `diagnostics`, `wipe-all`, or `debug`.

A compile-time exhaustiveness gate (`is_default_readable`) forces every new `ResourceKind` to make an explicit readable/excluded decision — adding a kind without updating it is a compile error.

## Middleware onion (local admin server)

`serve_with_shutdown` assembles the layers so the runtime order (outermost first) is:

```text
IP allowlist                   (outermost — rejects unauthorized peers before auth; inactive when empty)
  cache_control_no_store       (blanket Cache-Control: no-store on every response)
    require_token_configured_gate   (503 token gate — admin app only; skips static public paths)
      cli_token_pre_authn      (injects Role::Restricted + CliTokenIdentity for a recognized Bearer token)
        authz_layer            (route_layer — classifies + enforces the token's policy)
          handler
```

- **IP allowlist** (`edgion-controller/src/api/ip_allowlist.rs`) is mounted only when `allow_admin_ips` is non-empty; otherwise the configured default is allow-all.
- **`cache_control_no_store`** (`edgion-controller/src/api/mod.rs`) is cross-cutting and does not affect auth/authz semantics.
- **`require_token_configured_gate`** returns `503` + a startup WARN when no token (admin or cli) is registered, so a tokenless business request on an unconfigured controller yields `503` ("configure a token") rather than a bare `401`. It exempts the static public paths so `/api/v1/auth/status` and the probes stay reachable to report `Initializing`. **Admin app only** — never on the shared fed router (whose Center identity does not come from the token map).
- The public `GET /api/v1/auth/status` route is merged **after** the `authz_layer` `route_layer`, so it stays reachable without a token. `route_layer` (not `.layer`) means `authz_layer` never runs on the 404 fallback.

### authz_layer — the decision point

`edgion-controller/src/api/authz_middleware.rs`. `authz_layer` reads the `Role`, classifies the request, and calls `Authorizer::authorize`. Missing role → `401`; authenticated policy denial or unclassifiable route → `403`; allow → `next`. Static public paths bypass authz. There is no allow-all fallback or role-assignment middleware.

## CLI token workflow

1. **Mint**: `edgion-cli gen-token` — generates a CSPRNG token with an `edg_` prefix, prints both the token (for the client) and its SHA-256 (for the controller). The hash covers the **full `edg_`-prefixed string** (exactly what the Bearer header carries).
2. **Configure**: paste the SHA-256 into a `cli_tokens[]` entry with a `name` and an `rbac.allow` rule set. Schema + examples:
   [`../../02-features/02-config/00-controller-config.md` → `[cli_tokens]`](../../02-features/02-config/00-controller-config.md).
3. **Reload**: `POST /api/v1/reload` hot-swaps the token table — no restart. The local admin token survives the reload (re-merged by `Authorizer::apply`).
4. **Use**: `edgion-cli --token <edg_…>` (long-only; `-t` is taken by `--target`) or the `EDGION_TOKEN` env var, or persist it once with `edgion-cli login --token <edg_…>` (writes `~/.edgion/token`). Token precedence: `--token` → `EDGION_TOKEN` → `~/.edgion/token`.

The local admin token (operator credential) is a distinct concept: it is supplied to the *controller* via `EDGION_ADMIN_TOKEN` / the `admin_token` config field and maps to the allow-all wildcard policy. Operator walkthrough and the full security checklist live in the human-facing ops guide: `docs/en/ops-guide/federation-rbac.md#cli-token-rbac`.

## Observability

Denials increment `edgion_fed_rbac_denied_total` via `record_rbac_denied(verb, kind, source)` (`edgion-common/src/observe/fed_metrics.rs`). The `source` label is bounded to `center | cli_token | unknown` — the cli-token *name* is **never** a metric label (unbounded cardinality); it appears only in the structured deny log (`token_name` / `provider` fields). `source` is derived from the presence of `CliTokenIdentity` in the request extensions.

## Module map

| Concern | Path |
|---------|------|
| Authz engine (`Role` / `Verb` / `CenterPolicy` / `Authorizer` / `Decision` / admin-token entry) | `edgion-controller/src/authz/mod.rs` |
| authz_layer + inject_center_identity | `edgion-controller/src/api/authz_middleware.rs` |
| Request → `(verb, kind)` classifier | `edgion-controller/src/api/authz_classifier.rs` |
| Token authn (`cli_token_pre_authn`) + `CliTokenIdentity` | `edgion-controller/src/api/cli_token_middleware.rs` |
| 503 token gate + `/api/v1/auth/status` handler + onion assembly | `edgion-controller/src/api/mod.rs` |
| Built-in default Center policy | `edgion-controller/src/fed_sync/default_policy.rs` |
| Local admin token resolution (`EDGION_ADMIN_TOKEN` / `admin_token`) | `edgion-controller/src/cli/mod.rs` |
| Public paths (controller authz skip-set) | `edgion-controller/src/api/public_paths.rs` |
| Gateway OIDC JWKS/discovery provider | `edgion-gateway/src/plugins/http/openid_connect/jwks_provider.rs` |
| `cli_tokens` / `center.rbac` / `admin_token` config types | `edgion-controller/src/config/mod.rs` |

## Related docs

- [`02-admin-api.md`](02-admin-api.md) — Admin API endpoints + token authn request order.
- [`../../02-features/02-config/00-controller-config.md`](../../02-features/02-config/00-controller-config.md) — `admin_token` / `[center]` / `[cli_tokens]` schemas.
- [`../../02-features/02-config/03-auth-bootstrap.md`](../../02-features/02-config/03-auth-bootstrap.md) — token bootstrap + fail-close behavior matrix.
- [`../../04-review/architecture/fed-proxy-mtls-fail-close.md`](../../04-review/architecture/fed-proxy-mtls-fail-close.md) — federation mTLS fail-close + Center identity injection.
