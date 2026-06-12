---
name: local-auth-skip-paths-allowlist-and-superuser-claims-gate
description: Use when reviewing findings that operator-configured skip_paths can expose admin/business routes unauthenticated, or that assign_superuser_if_no_role grants Superuser to requests that skipped unified_auth; covers the fr-cauth-01 two-part fix.
---

# Operator `skip_paths` is allowlisted + Superuser grant requires `UnifiedAuthClaims` — FIXED

## Conclusion

Re-reports of the form "operator `[auth].skip_paths` / `[local_auth].skip_paths` accepts any path with no allowlist, so adding a business route (e.g. `/api/v1/cluster/httproute`) bypasses `unified_auth`; on the Controller `assign_superuser_if_no_role` Case 3 then grants `Superuser` → unauthenticated full admin; on the Center (no authz layer) the route is fully unauthenticated" are **stale** — closed as fr-cauth-01 (2026-05-29).

The fix is two independent gates (both landed):
1. **Config allowlist (Option B)** — `AdminAuthConfig::validate` and `LocalAuthConfig::validate` reject any `skip_paths` entry that is not exactly `/health`, `/ready`, `/metrics`, or a `/api/v1/auth/` prefix. A business route in `skip_paths` is now a startup error on both Controller and Center.
2. **Superuser-grant requires authentication proof (Option C)** — `assign_superuser_if_no_role` only inserts `Role::Superuser` when a `UnifiedAuthClaims` extension is present (i.e. `unified_auth` actually validated a token). A request with no `Role`, no `AuthBypass`, and no claims now fails closed (403) instead of being treated as an authenticated admin.

## Core Rationale

**1. The two-part defect (pre-fix)**

`UnifiedAuthState::from_configs` merged every `cfg.skip_paths` entry verbatim into the shared skip-set (`unified_auth/mod.rs`), and neither `validate()` inspected the contents. `unified_auth_middleware` early-returns `next.run(request)` for any `skip_paths` match with **no `UnifiedAuthClaims` inserted**. On the Controller, `assign_superuser_if_no_role` Case 3 then read "no Role + no AuthBypass" as "authenticated admin" and granted `Superuser` — but that invariant is false for a request that skipped `unified_auth` entirely. On the Center there is no `authz_layer`/`assign_superuser_if_no_role` at all, so a skipped business route is simply unauthenticated.

**2. Why both gates, not one**

- Option C alone closes the Controller escalation robustly (any future skip-set addition is fail-closed), but does **not** help the Center, which has no Superuser layer — a skipped Center business route stays unauthenticated.
- Option B alone closes both components at config-validation time, but leaves the Controller's Case 3 invariant fragile (it would silently re-break if a business route ever re-entered the skip-set — e.g. the test-mode `/api/v1/admin/wipe-all` entry at `from_configs`).
- Together: B closes the operator footgun for both components; C makes the Controller Superuser grant safe-by-construction regardless of how a request reached the route_layer.

**3. The allowlist set**

`/health`, `/ready`, `/metrics` are public probe/metrics endpoints; `/api/v1/auth/*` are the public auth endpoints (login/logout/status are already unauthenticated by design). Nothing else is a legitimate bypass target. The shared helper `skip_path_is_allowlisted` (`edgion-common/src/auth/config.rs`) is the single source of truth, reused by both config validators.

## Fix Suggestions Not Accepted

- **Remove `skip_paths` entirely (Option A)** — considered and rejected as the chosen fix because the blast radius is large (public config fields + `LocalAuthState`/`AuthMiddlewareState` fields + `secret.rs` + `jwt_middleware.rs` + `local_auth/middleware.rs` + docs) and it removes a legitimate operator knob for custom probe paths. The allowlist (B) achieves the same security outcome with a far smaller surface.
- **Widen the allowlist to arbitrary prefixes** — defeats the purpose; the danger is exactly a skipped path that equals a real business route.
- **Drop the `UnifiedAuthClaims` check in Case 3** — re-opens the escalation; the claims presence IS the authentication proof.

## Re-evaluation Triggers

Re-open only if:

- A new authentication path is added that authenticates a request without inserting `UnifiedAuthClaims` and without a `Role`/`AuthBypass` — Case 3 would then wrongly deny it (adjust the proof check, do not loosen it).
- A legitimate new unauthenticated endpoint is introduced that is neither a probe nor under `/api/v1/auth/` — extend `skip_path_is_allowlisted` deliberately, with its own review.

## Reference Cases

- fr-cauth-01 (tasks/bug), closed 2026-05-29
- Config allowlist: `edgion-common/src/auth/config.rs` (`skip_path_is_allowlisted`, `SKIP_PATHS_ALLOWLIST_ERR`, `AdminAuthConfig::validate`), `edgion-common/src/local_auth/config.rs` (`LocalAuthConfig::validate`)
- Superuser gate: `src/core/controller/api/authz_middleware.rs` (`assign_superuser_if_no_role` Case 3/4) + regression test `no_claims_no_role_is_denied_not_superuser`
- Docs: `skills/02-features/02-config/00-controller-config.md` → `[auth]` / `[local_auth]` `skip_paths` rows
