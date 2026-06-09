---
name: gateway-admin-auth-review
description: Review lock for the Gateway admin API bearer-token auth — the locked invariants (always-on, 503 fail-close, 401, constant-time compare, merged-router coverage, redacted token, probe stays open) and a note not to re-report the historical unauthenticated finding.
---

# Gateway admin API auth — review lock

The Gateway admin API (`gateway.admin_listen`, default `127.0.0.1:12001`) is
**authenticated**: a static bearer token enforced by `admin_auth_middleware`
(`src/core/gateway/api/admin_auth.rs`), layered on the fully-merged admin router.

Do **not** re-report the historical "Gateway admin API has no authentication"
finding — it was resolved by the design at
`docs/superpowers/specs/2026-06-08-gateway-admin-auth-design.md`.

Locked invariants (flag a regression if any is broken):
- Auth is **always on** (no escape hatch); enforced even on the loopback default.
- No token configured → **503** fail-close (not fail-open); startup WARN.
- Missing/wrong token → **401**; constant-time digest compare; `Authorization`
  value is never logged.
- The auth layer wraps the **merged** router, covering the integration-testing
  (destructive LinkSys) routes — not just `/configclient`.
- `gateway.admin_token` is `#[serde(skip_serializing)]` + redacted in `Debug`.
- The probe router (`:12000`, `/health` `/ready`) stays auth-free by design.

Known follow-up (NOT a finding): `allow_admin_ips` for the gateway admin port,
deferred to reuse the sister task's `ip_allowlist.rs` middleware.

## Integration-test plumbing (required when touching gateway admin auth)

Because the auth layer wraps the **merged** router, the `/api/v1/testing/*`
endpoints (access-log store, `testing/status`, `server-info`) also require the
bearer token. Every test-side consumer of the gateway admin API must present
`EDGION_GATEWAY_ADMIN_TOKEN`, or it gets 401 — which surfaces as access-log
assertion failures and 30s timeouts in retry loops, not as an obvious auth error.

Two touchpoints must stay in sync (both were missed once and caused ~20 failures):
- `examples/code/client/access_log_client.rs` — `AccessLogClient` reads
  `EDGION_GATEWAY_ADMIN_TOKEN` and attaches `bearer_auth` to **every** admin
  request (status, get/delete access-log, list/clear). Do not drop this.
- `examples/test/scripts/integration/run_integration.sh` — must `export`
  `EDGION_GATEWAY_ADMIN_TOKEN` **in its own process**. `start_all_with_conf.sh`
  also defaults it, but the runner invokes that script via command substitution
  (`output=$(...)`), a subshell whose `export` does **not** reach the runner or
  the `test_client` children it later spawns. The runner-level export is what lets
  the start script, the bash admin helpers, and `test_client` share one value.

Do **not** "simplify" by deleting the runner-level export as redundant with the
start script's — they live in different processes; removing it reintroduces the
401 cascade.

Known false positives (do NOT report these):
- "clap `env` feature is unused / dead — drop it from `Cargo.toml`." It is
  **required** by the validator binaries `examples/code/validator/cache_residue.rs`
  and `resource_diff.rs`, which use `#[arg(long, env = "EDGION_GATEWAY_ADMIN_TOKEN")]`
  to read the gateway admin token. A `src/`-only grep misses them. Removing the
  feature breaks those binaries.
- "`parse_bearer_token` accepts `Bearer<token>` without a separator." Fixed — it
  now requires a whitespace separator (`split_once(char::is_whitespace)`).
- The bare `gateway::api::serve()` wrapper passing `None` (fail-close) is
  intentional; it has no production callers (only `serve_with_shutdown` is wired).
