---
name: claims-access-log-filter
description: Read when reviewing store_claims_in_ctx / store_payload_in_ctx flowing JWT/JWE claims unfiltered into the access log / jwt_claims / oidc_claims / jwe_payload keys flatten into access-log JSON via ctx_map (gateway-plugins-03).
---

# gateway-plugins-03 — JWT/JWE claims unfiltered into access log

**Status / verdict**: fixed (gateway-plugins-03, 2026-05-25; the fix was later lost from the tree and re-landed 2026-06-14 — `entry.rs` had reverted to flattening `ctx_map` verbatim, so verify the `FilteredCtx` wrapper is actually present before trusting this record)

When reviewing findings of the form "`store_claims_in_ctx` / `store_payload_in_ctx` flow JWT/JWE claims unfiltered into the access log / `jwt_claims` / `oidc_claims` / `jwe_payload` keys flatten into access-log JSON via `#[serde(flatten)] ctx_map` / no allowlist or masking on `ctx_map` claim keys (gateway-plugins-03)": fixed (gateway-plugins-03, 2026-05-25).

The access-log projection of `ctx_map` now goes through `FilteredCtx` (`edgion-gateway/src/observe/access_log/entry.rs`), whose `Serialize` impl drops the `ACCESS_LOG_BLOCKED_CTX_KEYS` blocklist (`jwt_claims`, `oidc_claims`, `jwe_payload`) during serialization — allocation-free (no rebuilt map), since the access log runs on the data-plane path. Downstream plugins still read these keys via `session.get_ctx_var` (in-memory, per request); only the log projection is filtered. The issue's preferred Option B (config-driven `[access_log] ctx_keys` allowlist, default empty) is **not accepted** — default-empty would strip every CtxSet-written ctx key from the log, a regression of the documented `flatten` observability contract; the blocklist keeps all non-sensitive keys flattened with zero regression. Unit regression: `access_log_filters_sensitive_ctx_keys` in `entry.rs`. Any new `ctx_map` key that stores claims/payloads/secrets MUST be added to `ACCESS_LOG_BLOCKED_CTX_KEYS`. Re-reports of the unfiltered-flatten form are not accepted; verify the current `from_context` wraps `ctx_map` in `FilteredCtx` before re-reporting.
