# Admin API CORS: Same-Origin Deployment, No CORS Layer

## Conclusion

Edgion's Admin API (Controller `:5800` / Center `:5810`) **does not** attach a CORS layer. This is an **architectural decision**, not a missing config. If a review/audit report contains "Admin API CORS `allow_origin(Any)` is too open / should add allowlist" findings, treat them as "no code fix" unless a newly emerging cross-origin browser consumer can be pointed out.

## Reasons

The actual consumers of the Admin API and its deployment shape determine that CORS has no beneficiary in the current architecture:

- **`edgion-cli`** (Rust reqwest client): does not perform CORS checks
- **Prometheus `/metrics` scraping**: server-to-server, does not involve browser CORS
- **K8s `/health` `/ready` probes**: do not involve browser CORS
- **`edgion-dashboard` (browser UI, repo `ws3/edgion-dashboard`)**:
  - dev: Vite `server.proxy` proxies `/api`, `/health` to `localhost:5810`; the browser sees it as same-origin (see `edgion-dashboard/vite.config.ts`)
  - prod: the dashboard build artifact is embedded in the Controller binary, same-origin with the Admin API (see `edgion-dashboard/README.md` "Production environment" section)

There are 0 cross-origin browser consumers under the current deployment shape; attaching a CORS layer (whether permissive or allowlist) is preserving code and config surface for non-existent scenarios.

## Decision Rule

When a review/audit report contains the following patterns, close as "no code fix":

1. "Admin API CORS `allow_origin(Any)` / permissive" (H1 historical wording)
2. "Should introduce `[admin].cors_allowed_origins` config to restrict allowed origins"
3. "Production should mandatorily verify cross-origin source allowlist"
4. "Admin API Cookie/Bearer can be stolen by malicious websites via cross-origin requests"

Verify four points before closing:

- `src/core/common/api/mod.rs` contains no reference to `CorsLayer` / `tower_http::cors::*`
- The composition order in `src/core/common/api/compose.rs` does not contain a "CORS layer" step
- `Cargo.toml` does not contain the `tower-http` dependency (or contains it but does not enable the `cors` feature)
- `server.proxy` in `edgion-dashboard/vite.config.ts` still correctly forwards `/api` / `/health` to the backend (confirms dev still goes same-origin)

All four satisfied → not a real issue; point to the H1 FIXED (CORS layer removed) decision in `tasks/security-audit/04-admin-api.md` and close.

## Exception: When This Conclusion Does Not Apply

**The only exception**: if in the future a **separately-deployed-origin** browser consumer is introduced (e.g., the dashboard is split off from the Controller and deployed to an independent domain, or a third-party web tool integrates with the Admin API), this conclusion no longer holds and a redesign is needed — at that time, introduce a **precise origin allowlist** (not `Any`), with allowlist content drawn from real deployment needs.

Until that change happens, any "add CORS config field" suggestion should first be required to prove the existence of a cross-origin browser consumer.

## Background Notes

- Decision date: 2026-04-21
- Decision source: `tasks/security-audit/` H1 entry
- Related design document: `docs/superpowers/specs/2026-04-21-h1-admin-cors-removal-design.md`
- Related implementation record: `docs/superpowers/plans/2026-04-21-h1-admin-cors-removal.md`
- Same pattern: `skills/04-review/auth-timing/basic-auth-username-enumeration.md` (architectural close, refusing code fix suggestions)
