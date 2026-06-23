---
name: claims-access-log-filter
description: Read when reviewing store_claims_in_ctx / store_payload_in_ctx flowing JWT/JWE claims unfiltered into the access log / jwt_claims / oidc_claims / jwe_payload keys flatten into access-log JSON via ctx_map (gateway-plugins-03).
---

# gateway-plugins-03 — JWT/JWE claims unfiltered into access log

**Status / verdict**: fixed (gateway-plugins-03, 2026-05-25; blocklist `FilteredCtx` lost from the tree and re-landed 2026-06-14; **root-cause hardened 2026-06-21** — the default-on flatten was removed entirely in favor of a deny-by-default opt-in allowlist).

When reviewing findings of the form "`store_claims_in_ctx` / `store_payload_in_ctx` flow JWT/JWE claims unfiltered into the access log / `jwt_claims` / `oidc_claims` / `jwe_payload` keys flatten into access-log JSON via `#[serde(flatten)] ctx_map` / no allowlist or masking on `ctx_map` claim keys (gateway-plugins-03)": fixed.

**Current design (2026-06-21): deny-by-default allowlist.** The access-log projection of `ctx_map` goes through `AllowlistedCtx` (`edgion-gateway/src/observe/access_log/entry.rs`). A `ctx_map` entry is emitted **only** when its key appears in the matched route's opt-in allowlist (the `edgion.io/log-ctx-keys` annotation — or the `type: ctx` entries of the unified `edgion.io/log-keys` JSON annotation — on HTTPRoute / GRPCRoute, resolved once at conf-handler build time via `resolve_log_ctx_keys` and carried on `MatchInfo`/`GrpcMatchInfo.log_ctx_keys`). When a route opts nothing in (the default), **no** context variable is logged. The former "flatten every `ctx_map` entry minus a blocklist" behavior is gone — this removes the standing risk that a future plugin writing a secret into `ctx_map` would leak by default unless someone remembered to blocklist it.

The `ACCESS_LOG_BLOCKED_CTX_KEYS` blocklist (`jwt_claims`, `oidc_claims`, `jwe_payload`) is **kept as a secondary, defense-in-depth guard**: a hard-blocked key is dropped even if an operator explicitly allow-lists it, so claims/payloads can never be logged by mistake. Downstream plugins still read these keys via `session.get_ctx_var` (in-memory, per request); only the log projection is gated. Serialization stays allocation-free (no rebuilt map), since the access log runs on the data-plane path.

This supersedes the previously-rejected "Option B" (config-driven `[access_log] ctx_keys` allowlist, default empty). That was rejected on the grounds that default-empty regresses the documented `flatten` observability contract; that contract was **intentionally retired** here in favor of explicit opt-in. Unit regressions: `access_log_omits_ctx_without_allowlist` and `allowlisted_ctx_projects_only_allowed_non_blocked_keys` in `entry.rs`. Any new `ctx_map` key that stores claims/payloads/secrets SHOULD still be added to `ACCESS_LOG_BLOCKED_CTX_KEYS` (belt-and-suspenders). Re-reports of the unfiltered-flatten form are not accepted; verify `from_context` builds `AllowlistedCtx` gated by `log_ctx_keys` before re-reporting.
