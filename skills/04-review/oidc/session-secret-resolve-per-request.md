---
name: session-secret-resolve-per-request
description: Use when reviewing OIDC findings claiming `resolve_session_secret` / `resolve_client_secret` performs a per-request `RwLock` acquisition + `Secret::clone` on the data-plane hot path when `sessionSecretRef` / `clientSecretRef` is used; documents why the described hot-path cost does not exist in any real deployment and why caching into `OnceLock<Arc<str>>` is not accepted.
---

# OIDC `resolve_session_secret` / `resolve_client_secret` "per-request RwLock+clone" — NOT-A-BUG (premise does not match real behavior)

## Conclusion

`resolve_session_secret` and `resolve_client_secret` (`src/core/gateway/plugins/http/openid_connect/session_cookie.rs`) are **not** a data-plane hot path. The happy-path short-circuit on `resolved_session_secret` / `resolved_client_secret` already eliminates the `get_secret()` fall-through in normal operation; the degraded path where the fall-through runs reads from a permanently-empty global store, so there is no `Secret::clone()` to eliminate and no `RwLock` contention with TLS / other consumers.

Fix suggestions of the form "cache the session/client secret into an `OnceLock<Arc<str>>` / `OnceCell<Arc<str>>` on the plugin instance so every authenticated request avoids the lock + allocation + clone" are **not accepted** — they add a caching layer with refresh / staleness questions to eliminate a cost that does not exist.

## Core Rationale

**1. The happy path never reaches `get_secret()`**

Both functions short-circuit on `self.config.resolved_*_secret` being `Some(non_empty)` (`session_cookie.rs:12-16,58-62`). The controller's `EdgionPluginsHandler::parse` unconditionally calls `cred::resolve_openid_connect` (`handlers/edgion_plugins.rs:96`) before pushing the config to the gateway; that resolver reads the K8s Secret named in `sessionSecretRef` / `clientSecretRef`, runs the value through `read_secret_utf8` (which only returns `Some(non-empty)` or `None`), and writes it into `resolved_session_secret` / `resolved_client_secret` (`handlers/plugins/resolve_credential/openid_connect.rs:33,77`). In every deployment where the Secret exists and carries the expected key, the gateway-side OIDC plugin receives `Some(<non_empty_value>)` and the short-circuit returns at line 14 / 60 — no `get_secret()`, no lock, no `format!`, no `HashMap` lookup, no `Secret::clone()`.

**2. The fall-through path reads from a permanently-empty store**

The fall-through calls `get_secret(Some(ns), &name)` (`session_cookie.rs:25,70`), which delegates to `get_global_secret_store().get(..)` on the static `GLOBAL_SECRET_STORE` (`secret_store.rs:90,102-104`). Grep across the whole tree shows **zero writers to `GLOBAL_SECRET_STORE`** — every real Secret write target is `ControllerRuntime::secret_store` (`runtime.rs:46,83,147`; `pipeline.rs:284`; `handlers/edgion_plugins.rs:290`; `handlers/secret.rs:162,177,204`; `handlers/edgion_tls.rs:638`), a *different* `SecretStore` instance bound to the controller runtime. The static is therefore always an empty `HashMap`, and the fall-through cost is bounded by:

1. one uncontended `parking_lot::RwLock::read()`,
2. one small `format!("{}/{}", ns, name)` allocation,
3. one `HashMap::get` miss → `None`,
4. an `Err("Secret <ns>/<name> not found")` returned to the caller.

No `Secret::clone()` ever happens (the map is empty, no entry to clone). The "BTreeMap deep-copy of secret bytes per request" cost the finding warns about is not reachable.

**3. The "shared with TLS and other secret consumers" claim is also wrong**

TLS, the Secret handler, `resolve_credential`, and `edgion_tls` all write to `runtime.secret_store`, never to `GLOBAL_SECRET_STORE`. The two `SecretStore`s share a type and an `RwLock<HashMap>` shape but they are distinct instances, so contention between TLS and OIDC on `GLOBAL_SECRET_STORE` is impossible by construction.

**4. The degraded path is *correctly* per-request — caching would hide the bug**

If `resolved_session_secret` is `None` (controller could not resolve: Secret missing, missing key, or `ReferenceGrant` denied), every authenticated request returns `Err` from `resolve_session_secret`, which surfaces as `Missing session secret: Secret X/Y not found` at the OIDC verify path (`session_cookie.rs:215-218`) and a 401 to the client. This is the *correct* behavior — it fails closed per request, so the operator notices and fixes the misconfiguration. Caching the `Err` into an `OnceLock` would freeze that failure for the lifetime of the plugin instance even after the Secret appears (the controller already triggers a fresh `EdgionPlugins` resync when the missing Secret arrives, so a "cache" that captures the failure once is strictly worse than the current re-read-on-every-request behavior). Caching only successful resolutions, on the other hand, would only ever activate in the degraded path that does not happen in practice (see (2): the fall-through reads an empty store), so the cache would never hold a value either.

## Fix Suggestions Not Accepted

- Cache `resolved_session_secret` / `resolved_client_secret` into `OnceLock<Arc<str>>` / `OnceCell<Arc<str>>` at plugin construction time — pays for refresh-on-resync plumbing to eliminate a cost the happy path already skips (short-circuit on `Some(non_empty)`).
- "Resolve at first successful resolution into an `Arc<str>` stored on the plugin instance" — the only path that exercises `get_secret()` is the degraded path against an empty global store; there is no "first successful resolution" to cache.
- Reframe `get_secret()` as ArcSwap-backed to avoid the lock — would be true churn (the store *is* `RwLock<HashMap>`), but does not address an actual hot-path cost; and the static is never read on the happy path.
- Treat the `Some("")` guard at lines 13 / 59 as the bypass channel — already covered by [oidc/resolved-secret-empty-fallthrough.md](resolved-secret-empty-fallthrough.md); `Some("")` is unreachable through every real path.

## Re-evaluation Triggers

Re-open only if:

- A writer is added to `GLOBAL_SECRET_STORE` (i.e. the gateway binary starts populating it from a Secret sync source), making `get_secret()` actually return a `Some(Secret)` on the fall-through path — at that point the `Secret::clone()` cost becomes real and the caching argument applies.
- The controller's `cred::resolve_openid_connect` is removed or moved behind a feature flag so the gateway no longer receives `resolved_session_secret = Some(non_empty)` in the happy path, forcing every OIDC-protected request through `get_secret()`.
- `session_cookie.rs` is restructured so `resolve_session_secret` / `resolve_client_secret` is invoked from a colder path (e.g. plugin warmup) — then the per-request framing in the finding stops being load-bearing and the discussion is about plumbing cleanup, not a hot-path fix.

## Reference Cases

- F-015 (Round B follow-up, closed 2026-05-27) — `tasks/plugins/plugins-http-F015-oidc-session-secret-resolved-per-request.md`, closed as accepted-tradeoff with no code change.
- Source: `src/core/gateway/plugins/http/openid_connect/session_cookie.rs:11-16,57-97,215-218`
- Controller resolver: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/openid_connect.rs:23-112`
- Empty global store: `src/core/controller/conf_mgr/sync_runtime/resource_processor/secret_utils/secret_store.rs:84-104` (no writers tree-wide)
- Sibling skill: [oidc/resolved-secret-empty-fallthrough.md](resolved-secret-empty-fallthrough.md)
