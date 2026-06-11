# Task: Build the dashboard frontend into edgion-center

**Profile:** single-file / docs
**Status:** todo (design draft — not yet started)
**Scope:** **edgion-center only.** Controller does not support embedding the UI for now and
is explicitly out of scope. (The frontend README and `compose.rs` mention "embedded into
the Controller" as the original intent — that is *not* what this task does; only Center
embeds the UI here.)
**Frontend repo:** `/Users/caohao/ws1/edgion-dashboard` (React 18 + TS + Ant Design + Vite)
**Boundary note:** this embedded **basic** dashboard stays in the lean Federation Hub (Center
core). The **rich governance UI** (RBAC/audit/multi-tenant) belongs to the separate optional
**Management Plane** — see `center-split-hub-management-plane.md`.

## Goal

Serve the `edgion-dashboard` web UI directly from the `edgion-center` binary so that
the management UI is reachable same-origin on Center's Admin API listener
(`http_addr`, default `12201`) without a separate web server.

## Key conclusion: this is static hosting, NOT an nginx reverse proxy

In a typical SPA-behind-nginx deployment nginx does two jobs:

1. Serve static assets (html/js/css with correct MIME, cache headers, gzip).
2. Reverse-proxy `/api` to the backend.

Center only needs **job 1 plus SPA history fallback** (serve `index.html` for unknown
non-`/api` paths so a hard refresh on a client-side route does not 404).

Job 2 (reverse proxy) is **not needed**, because this is a same-origin embed:

- The dashboard's axios client uses a relative `baseURL = '/api/v1'`
  (`edgion-dashboard/src/api/client.ts`) — no hard-coded backend host.
- `vite.config.ts`'s `server.proxy` is **dev-only**; it is not part of the production build.
- `src/core/common/api/compose.rs` already documents the intent: "in production the
  dashboard is embedded and deployed same-origin … The Admin API does not mount a CORS
  layer." No CORS, no reverse proxy required.

Note: Center's existing `/api/v1/proxy/{controller_id}/*rest`
(`src/core/center/api/mod.rs`) is an Admin-request forwarder to downstream controllers —
unrelated to serving frontend assets. Do not conflate the two.

## Current state

- No static-asset serving anywhere in `src/` today: no `rust-embed` / `include_dir` /
  `tower-http::ServeDir`, and no related Cargo dependency.
- Center's `router()` (`src/core/center/api/mod.rs`) is all `.route("/api/v1/...")` with
  **no fallback**.
- Center config (`src/core/center/config/mod.rs`, ~270 lines) has **no** web/static dir field.

## Packaging decision: hybrid (embed by default + filesystem override)

Two viable approaches were weighed:

| | A. rust-embed (compile into binary) | B. bake `dist/` into the Docker image + `ServeDir` |
|---|---|---|
| Single artifact | yes | no (binary + `dist/`) |
| Fits the `kubectl cp` binary hot-patch workflow | yes — UI updates atomically with the binary | no — hot-patching the binary does not update the UI |
| Bare-metal / non-Docker deploy | self-contained | must ship and configure `dist/` separately |
| Front/back version consistency | always matched | can skew |
| Patch frontend without recompiling Rust | no (or via build.rs) | yes (rebuild one image layer) |
| Rust-side complexity | medium (must `npm build` before `cargo build`) | low (point `ServeDir` at a dir) |
| New config needed | none | a `web_dir` field + missing-dir degradation |

**Decision: hybrid, and the asset source is a first-class runtime requirement —
configurable, default = bundled.** Same binary can serve either the embedded assets or a
filesystem directory; when nothing is configured it serves the embedded copy.

### Asset-source resolution (runtime, highest precedence first)

1. `EDGION_WEB_DIR` env var set → serve that directory via `ServeDir` (override / dev / emergency patch).
2. `web_dir` config key set → serve that directory via `ServeDir`.
3. neither set → serve the **embedded** assets (the default — "bundled together").
4. assets not compiled in (`embed-dashboard` feature off) **and** no dir configured →
   pure-API mode (no UI; document whether unknown paths return 404 or a clear message).

Two independent layers, do not conflate them:

- **Compile-time** (`embed-dashboard` Cargo feature): whether the binary *contains* the
  assets. **On for Center only**; off for Gateway and Controller (Controller embedding is
  out of scope for now). This keeps the other binaries slim.
- **Runtime** (`web_dir` / `EDGION_WEB_DIR`): *which* source is served. Default unset →
  embedded. This is the user-facing "configurable, default bundled" knob.

Rationale:

- The runtime is built around `kubectl cp <binary>` hot-patch + restart-loop
  (`docker/Dockerfile`, `docker/Dockerfile.runtime`). Embedding keeps the UI on that same
  fast release path; a UI living outside the binary would be excluded from it.
- The override path recovers every advantage of Docker-baking on demand (dev hot-reload of
  the frontend; emergency frontend patch without recompiling Rust; pure image-baking
  deploy if ever wanted).
- Gateway must not carry the UI: gate the embed behind a Cargo feature
  (e.g. `embed-dashboard`) enabled only for Center (and Controller).

### On the "more vulnerability fixes" concern

Packaging does **not** change the number or severity of frontend CVEs — the same JS ships
to the same browser either way; it only changes the **rebuild cost** per patch (rust-embed
forces a Rust recompile; image-baking rebuilds one layer). Mitigations that make the
embed downside negligible:

- Most `npm audit` findings are **dev dependencies** (vite, eslint, plugins) that never
  ship in the `vite build` output and are irrelevant at runtime regardless of packaging.
  Track real exposure with `npm audit --omit=dev` (runtime deps only: react, react-dom,
  antd, axios, react-router-dom, js-yaml, zod).
- The `EDGION_WEB_DIR` override is the emergency lever: swap a fixed `dist/` at the
  directory level without rebuilding the binary.
- A single binary gives **unified SBOM / provenance** (no binary-vs-`dist/` version skew),
  which is marginally better for supply-chain auditing.
- This is an authenticated, same-origin **admin** panel behind `unified_auth`, not public
  user traffic — the realistic frontend (XSS) risk weight is lower than a public SPA.
- Ops habit: enable Dependabot/Renovate on the dashboard repo scoped to runtime deps;
  batch routine bumps into image releases, use the override for urgent CVEs.

## Scope: the dashboard has no Center mode today — chosen path is "proxy passthrough"

Investigated `edgion-dashboard/src`: it is a **Controller-only** resource UI.

- Header is hard-coded "Edgion Controller" (`src/components/Layout/MainLayout.tsx`);
  Dashboard page shows "Controller: Running" (`src/pages/Dashboard/index.tsx`).
- Only resource implemented is HTTPRoute CRUD against the generic `/api/v1/...` resource
  API (`src/api/resources.ts`).
- **Zero** Center concepts present: no controllers list, no clusters, no
  cluster/service-region-routes, no failover/consistency, no global IP restrictions, no
  use of `/api/v1/proxy/...`.

Three candidate scopes were considered:

1. **Embed-as-is now, Center pages later** — static hosting only; the UI loads but every
   API call 404s against Center (Center does not expose the Controller resource routes).
   Ships a broken UI; rejected.
2. **Reuse `/api/v1/proxy/{controller_id}/*`** — the embedded dashboard manages a
   downstream controller selected through Center's proxy. Smallest change, immediately
   usable. **CHOSEN.**
3. **Build Center-specific pages/API first** — native region-route aggregation / failover /
   consistency UI. Large standalone frontend project; deferred as the follow-up increment.

### Decision: Option 2 (proxy passthrough), with Option 3 as a later increment

Verified feasible against the code: `proxy_handler` + `ProxyForwarder.forward`
(`src/core/center/api/mod.rs`, `src/core/center/proxy/mod.rs`) forward the full request
(method, path, headers, body) over the gRPC stream to the target controller, which runs it
against its own Admin API and returns the response verbatim. `controller_id` encodes `/`
as `~` to dodge browser URL decoding.

Why Option 2:

- Only path that yields a **working** UI with **minimal** frontend change. Option 1 ships a
  broken UI; Option 3 blocks embedding on a large new frontend.
- Connects what already exists on **both** sides: Center's proxy forwarder + the full
  Controller dashboard. Net-new frontend work ≈ a controller selector + a baseURL prefix.
- Matches Center's purpose: a federation operator manages downstream controllers from one
  place.
- Does **not** preclude Option 3 — native Center aggregation pages can be layered on later.

Frontend changes for Option 2:

- Fetch the downstream controller list from Center's `/api/v1/controllers`; add a
  top-level controller selector.
- Change the axios `baseURL` from `/api/v1` to
  `/api/v1/proxy/{selectedController}/api/v1` (encode `/` in the controller id as `~`).

### Option 2 caveats

1. **Proxy auth trust boundary — RESOLVED (verified in code).** The browser's forwarded
   token is **not** what authenticates at the controller. The model is:

   - **Authentication boundary = the mTLS gRPC stream.** `fed_client` only connects after
     the `resolve_mtls_or_refuse` gate, so Center is an mTLS-authenticated peer.
   - The controller runs each proxied request through `fed_router`
     (`src/core/controller/cli/mod.rs` ~589), which is built **without** `unified_auth`.
     Instead it applies `inject_center_identity`
     (`src/core/controller/api/authz_middleware.rs:183`, injects `authorizer.center_role()`)
     + `authz_layer`. The forwarded browser `Authorization`/cookie is effectively ignored
     for authn on the controller side.
   - Net: a proxied request executes on the controller with **`center_role()`** permissions,
     authorized by `authz_layer` — "arrived over the trusted mTLS stream = you are Center".
   - The dispatch is in-process: `admin_router.oneshot(req)`
     (`src/core/controller/fed_sync/fed_client/mod.rs` ~522), no extra network hop.

   **Consequence for the dashboard:** the user authenticates **at Center**
   (Center's `unified_auth` + IP allowlist guard the `/api/v1/proxy/...` route, which is a
   normal protected business route). The browser only needs a valid **Center** session — it
   does **not** handle controller credentials, and Center does **not** inject per-controller
   tokens. No frontend auth work beyond logging into Center.

   **Residual — DECIDED: proxied dashboard gets the controller's normal admin authority.**
   The interactive proxy path must authorize exactly as the controller authorizes a normal
   authenticated admin — i.e. full access (`Role::Superuser`), **not** the restricted
   `center_role()`. Rationale: the user is already authenticated at Center, and the mTLS
   stream proves the caller is Center, so the controller treats proxied admin requests like
   any directly-authenticated admin (in the controller a valid `unified_auth` token →
   `assign_superuser_if_no_role` → `Superuser`).

   Context that makes this clean and safe to implement:

   - The default `center_role()` (`default_center_policy`) is deliberately **read-mostly**
     (read on non-Secret kinds; write only on PluginMetaData; **Secret fully denied**). That
     restriction exists for automated fed traffic and is **kept as-is**.
   - The **automated fed-sync watch** path authorizes via `center_role()` **directly in
     code** (`fed_client/mod.rs:606`), separate from the **interactive proxy** path (which
     dispatches through `admin_router.oneshot`). So the proxy path's effective role can be
     raised to `Superuser` **without touching** the sync restriction — clean isolation.

   Implementation direction: for the **HttpProxy** dispatch only, inject `Role::Superuser`
   instead of `center_role()` (today `inject_center_identity` injects `center_role()` for
   the whole `fed_router`). Keep `authz_layer` in place; just change the injected role on
   the proxy branch. Watch/sync stays on `center_role()`.

   Security note (knowingly accepted, confirm intent): this makes **Center a full-control
   plane over every downstream controller, including read/write of Secrets**. That follows
   from "Center is the federation control plane", but it is broader than today's fed default
   which intentionally excludes writes and Secret access. Center's own auth gate
   (`unified_auth` + IP allowlist on `/api/v1/proxy/...`) and the mTLS stream are the
   compensating controls.

2. **Coverage:** Option 2 only covers managing a **single downstream controller**. Center's
   own cross-cluster value-add (region-route aggregation, failover fan-out, consistency,
   global IP restrictions) is **not** covered — that is the Option 3 follow-up.

## Implementation outline (scope = Option 2)

Backend (static hosting + packaging):

1. Add `rust-embed` (feature-gated `embed-dashboard`) + `tower-http` `ServeDir` for the
   override path; a build step to produce `edgion-dashboard/dist/` before `cargo build`.
2. Add a fallback handler to Center's router: serve embedded/override asset by path with
   correct MIME + cache headers; fall back to `index.html` for unknown non-`/api` routes.
3. **Auth ordering (critical):** `compose_admin_routes` (`src/core/common/api/compose.rs`)
   enforces "the returned Router is final — no further `.route()`/`.layer()`", or the auth
   middleware is bypassed. Decide whether static assets sit inside or outside auth: the
   login page and its JS/CSS must load **unauthenticated**, yet the fallback must not leak
   API auth. Design this explicitly.
4. Add `web_dir`/`EDGION_WEB_DIR` config + missing-dir degradation (pure-API mode vs 503).
5. Docker: only the **Center** image needs the UI — keep Gateway and Controller images unchanged.

Frontend (Option 2 proxy passthrough — in `edgion-dashboard`):

6. Add a controller selector sourced from Center's `/api/v1/controllers`; persist the
   current selection (store/route).
7. Make the axios `baseURL` dynamic:
   `/api/v1/proxy/{selectedController}/api/v1` with `/` → `~` in the controller id.
   When served by Center (e.g. probe `/api/v1/server-info` → `{"mode":"center"}`) use the
   proxy baseURL; the existing standalone/dev mode against a Controller (Vite proxy to
   `:5800`, plain `/api/v1`) is unchanged. Controller does not embed the UI in this task.
8. Update the hard-coded "Controller" labels (header, dashboard status) to reflect
   Center + the selected controller.

Pre-design verification:

9. Proxy auth trust boundary — **resolved** (see caveat 1): browser authenticates at Center;
   proxied requests run on the controller as `center_role()` over the mTLS stream. No
   frontend credential handling needed.
10. **Proxy authz role (DECIDED, see caveat 1):** raise the **HttpProxy dispatch** path to
    `Role::Superuser` (controller's normal admin authority), leaving the automated fed-sync
    `center_role()` restriction untouched. Backend change in
    `src/core/controller/fed_sync/fed_client/mod.rs` / the `fed_router` proxy branch
    (`src/core/controller/cli/mod.rs`); not frontend. Add a test asserting a proxied write
    (e.g. HTTPRoute create) succeeds while sync stays restricted.

## Progress

### Backend static-hosting foundation — DONE (steps 1–4)

Implemented and verified (fmt, `cargo check` default + `--features embed-dashboard`,
clippy clean on new code, `make check-agent-docs`, 8/8 web tests pass):

- **Cargo**: `embed-dashboard` feature (off by default) → optional `rust-embed`. No
  always-on deps added (MIME handled by a small manual map so filesystem serving works
  without the feature).
- **`src/core/center/api/web.rs`**: `WebSource` (Dir | Embedded) with precedence
  `EDGION_WEB_DIR` > `web.dir` > embedded > none; `serve()` fallback handler with
  content-type + cache-control (`index.html` no-cache, hashed `assets/*` immutable), SPA
  shell fallback, and `/api/*` miss → 404 (never the shell). Path-traversal sanitized.
- **`src/core/center/config/mod.rs`**: `WebConfig { dir: Option<String> }` under `web`.
- **`src/core/center/cli/mod.rs`**: public SPA fallback mounted **after**
  `compose_admin_routes` (auth-protected `/api` routes added before stay protected),
  inside the IP-allowlist layer. Pure-API mode when no source resolves.
- **Dashboard NOT vendored**: `web/` is gitignored (not committed). `build.rs` generates a
  minimal placeholder `web/dashboard/index.html` only when building `--features
  embed-dashboard` (so the feature compiles standalone); the real `dist/*` is downloaded and
  staged there by `build-image.sh` at image-build time and never enters the repo.
- **Security guard test** `fallback_is_public_while_api_stays_protected`: asserts asset/SPA
  paths are public while `/api/v1/*` returns 401 unauthenticated and unknown `/api` → 404.
  Locks the auth-ordering contract.

### Build/CI wiring — DONE (step 5), `bash -n` clean (NOT run end-to-end; needs Docker+npm)

Decision (user): **embed replaces the nginx all-in-one-center**. The existing frontend
download/build machinery already existed (`prepare_dashboard` = local sibling
`../edgion-dashboard` or clone from GitHub; `compile_frontend` = npm build in `node:20`),
wired to a separate nginx image — that path is now retired for Center.

- **`build-image.sh`**:
  - `BINARIES="gateway controller center"`; `EMBED_WEB_DIR="web/dashboard"`.
  - New `build_frontend_into_web()` (reuses `prepare_dashboard`; npm build in docker; stages
    `dist/.` → `web/dashboard/`) + `cleanup_embed_staging()` (restores committed placeholder,
    drops staged output). Frontend built **once** before the per-arch loop when `center` is a
    target.
  - `compile_binaries()`: gateway/controller/cli built as before (no feature, slim); then a
    **separate** `cargo build --bin edgion-center --features "${FEATURES},embed-dashboard"`
    when `center` ∈ BINARIES. Packaged via the existing `Dockerfile.runtime` (no change —
    UI is in the binary).
  - `--all-in-one-center` **retired**: now prints a deprecation error pointing at the standard
    center image. Usage/examples updated. (`all_in_one_controller_gateway` path untouched.)
- **`docker/Dockerfile.all-in-one`** + **`docker/all-in-one-entrypoint.sh`**: center branches
  removed; controller-gateway mode intact.
- **`docker/nginx/all-in-one-center.conf`**: deleted (its job — static serve + SPA fallback +
  `/api`→:12201 proxy — is now done in-process by `web::serve` same-origin).

Caveats / minor follow-ups:
- Not run end-to-end here (needs Docker + npm). Validate with a real
  `./build-image.sh` (or `BINARIES=center ./build-image.sh --compile-only`).
- `Dockerfile.runtime` `EXPOSE` lists gateway/controller ports, not Center's
  (12201/12251/12200/12290). EXPOSE is informational only (does not restrict binding), so
  non-blocking — add Center ports if desired.
- A few inert `all_in_one_center` no-op lines remain in `build-image.sh` (flag is never set
  true now); harmless, can be tidied later.

### Frontend (steps 6–8) — DONE in `edgion-dashboard` (separate repo), `npm run build` passes

- `src/api/center.ts` (new): `centerClient` (fixed `/api/v1`), `getServerMode()` (probes
  `/server-info`), `listControllers()`, `encodeControllerId` (`/`→`~`),
  `setActiveController()` (rewrites `apiClient.defaults.baseURL` to
  `/api/v1/proxy/{id}/api/v1`).
- `src/store/center.ts` (new): zustand store — `init()` detects mode, in center mode loads
  controllers + restores localStorage selection; `selectController()`; persistence.
- `src/App.tsx`: runs `init()` on mount, gates render until initialized (baseURL set before
  first query).
- `MainLayout.tsx`: mode-aware title (Center/Controller); center mode adds a controller
  `<Select>` (online Badge); switching calls `queryClient.clear()` to refetch.
- `Dashboard/index.tsx`: system-info card shows mode + active controller.
- **Also fixed 16 pre-existing TS errors** that blocked `npm run build` (Zod v4
  `z.record(k,v)` two-arg in `common.ts` — root cause of the HTTPRoute label-type cascade;
  react-query v5 `cacheTime`→`gcTime`; unused imports/vars; a `K8sResource`→`HTTPRoute`
  prop cast). Verified: `npx tsc --noEmit` 0 errors; `npm run build` produces `dist/`.
- Not committed (separate repo — left for your review/commit). Note: until Authz step 10,
  the proxied dashboard is **read-only** for most resources (see below).

### Remaining

- **Authz — DECIDED: two independent layers; Center stays "login = admin" for now.**
  - **controller→center** (fed RBAC / `center_role`): a separate inter-system concern (the
    ceiling). Configured on the controller side via `FedRbacConfig`; not Center's code.
  - **center→ops** (Center's own per-user authz): **not built** — any authenticated user
    (Okta/OIDC or `local_auth`) is a full Center admin. `unified_auth` does authn only;
    Center has no authz layer. The `UnifiedAuthClaims` (with full claims for scope/role) are
    available but unused — per-user RBAC can be added later if needed.
  - **Compensating control = audit log** (since everyone is an admin): see
    `tasks/todo/center-audit-log.md`.
  - **Proxy write-through:** still capped by the default `center_role()` (read-mostly) until
    the `FedRbacConfig` is widened (config-only) or step 10 Superuser elevation is done. The
    embedded dashboard is effectively read-only for most resources until then.

## Deployment & TLS (no code change — existing config covers it)

The embedded dashboard and `/api` are served **same-origin** on the Center admin listener
(`server.http_addr`, default `0.0.0.0:12201`). HTTPS is Center's own responsibility via
`server.admin_tls` (cert/key) — there is no separate frontend port.

Two supported topologies, both require Center to terminate TLS itself:

1. **K8s + L4 NLB (TCP passthrough).** The NLB does **not** terminate TLS, so Center **must**
   enable `admin_tls`. NLB maps external `443` → Center `12201` at L4; TLS terminates at
   Center. Container runs non-root on `12201` (>1024) — no privileged-port issue.
2. **VM with its own IP.** Center binds directly, also HTTPS via `admin_tls`. If binding
   `http_addr: 0.0.0.0:443` is desired, that is a privileged port (<1024): run as root or
   grant `CAP_NET_BIND_SERVICE` (e.g. systemd `AmbientCapabilities`). Otherwise keep `12201`.

Production rule: **`admin_tls` must be set.** Consequences handled in code already:

- The dashboard's relative `/api/v1` baseURL inherits the page's `https://` scheme → no
  mixed-content, no frontend config.
- `local_auth.cookie_secure` defaults to `true` → login cookie carries `; Secure` under
  HTTPS automatically.
- Startup diagnostic `admin_tls_cookie_warning(admin_tls_present, cookie_secure)` (wired in
  `cli/mod.rs`) warns on an `admin_tls` / `cookie_secure` mismatch.

`EXPOSE` in `docker/Dockerfile.runtime` is informational only (actual exposure is the k8s
Service / NLB, or nothing on a VM); it does not affect either topology.

## References

- Center API/router: `src/core/center/api/mod.rs`
- Shared admin composition + auth contract: `src/core/common/api/compose.rs`
- Center config: `src/core/center/config/mod.rs`
- Docker runtime + hot-patch workflow: `docker/Dockerfile`, `docker/Dockerfile.runtime`
- Frontend: `/Users/caohao/ws1/edgion-dashboard` (`src/api/client.ts`, `vite.config.ts`,
  `README.md`)
