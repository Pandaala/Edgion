---
name: resolved-secret-empty-fallthrough
description: Use when reviewing OIDC resolve_session_secret / resolve_client_secret findings claiming Some("") "bypasses" the resolved secret and falls through to the Secret store; documents why the two-level Some(..) + !is_empty() check is intentional defensive coding and Some("") is unreachable.
---

# OIDC `resolve_session_secret` / `resolve_client_secret` `Some("")` Fallthrough — NOT-A-BUG (intentional defensive pattern)

## Conclusion

`resolve_session_secret` and `resolve_client_secret` (`edgion-gateway/src/plugins/http/openid_connect/session_cookie.rs`) deliberately use a two-level check — `if let Some(ref resolved) = self.config.resolved_session_secret { if !resolved.is_empty() { return Ok(resolved.clone()); } }` — so that an empty resolved secret falls through to the `session_secret_ref` / `client_secret_ref` Secret-store path instead of being used as a cipher key. This is **correct, safe behavior**, not a bug.

Fix suggestions of the form "`Some("")` silently bypasses the already-resolved secret", "flatten to `Option::filter` / `match Some(v) if !v.is_empty()` to remove the ambiguity", or "add a build-time validation rejecting an explicitly-set empty resolved secret" are **not accepted** (cosmetic churn against an unreachable state).

## Core Rationale

**1. `Some("")` is unreachable through every real path**

- **Controller resolution**: `resolved_session_secret` / `resolved_client_secret` are populated only by `read_secret_utf8` (`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/mod.rs:33`), which returns `None` for any value where `value.trim().is_empty()`. It yields `Some(non-empty)` or `None` — never `Some("")`. (`resolve_credential/openid_connect.rs` resets both to `None` first, then assigns only from `read_secret_utf8`.)
- **Wire format**: the field carries `#[serde(skip_serializing_if = "Option::is_none")]`; the controller only ever serializes a non-empty value, so the gateway never receives `Some("")` over ConfigSync.
- **CRD input**: the field is `#[schemars(skip)]` ("Runtime field, controller-populated"), so it is absent from the OpenAPI schema and K8s structural-schema pruning strips any user-supplied value — an operator cannot inject `Some("")`.
- **Default impl**: `OpenidConnectConfig::default()` sets both to `None`.

**2. Even if `Some("")` occurred, the fallthrough is the *safe* direction, not a "bypass"**

The inner `!is_empty()` guard *prevents* using an empty string as the AES-256-GCM key (`session_cipher` derives the key via `Sha256::digest(secret.as_bytes())`; an empty secret would still produce a 32-byte digest and silently "work" as a weak fixed key). Falling through to `*_secret_ref` is desirable: that path itself rejects empty secrets (`session_cookie.rs:93-95` / `:50-52`, `<value>.trim().is_empty()` → `Err`). The finding's "bypass already-resolved secret" framing implies a vulnerability, but the code is closing the dangerous case (empty cipher key), not opening one.

**3. The claimed "unnecessary K8s reads on every request" harm is moot**

`get_secret()` reads from the controller's in-memory resource store, not a live K8s API call; and the precondition (a buggy plumbing path that sets `Some("")`) does not exist per reason 1.

## Fix Suggestions Not Accepted

- Flatten to `Option::filter(|s| !s.is_empty())` / `match Some(v) if !v.is_empty()` — pure cosmetic refactor; the behavior is already correct and the awkward-looking shape encodes the intentional "empty resolved → fall through to ref" semantics for both functions identically.
- Add a plugin-build-time validation rejecting an explicitly-set empty resolved secret — guards a state that cannot be produced by the controller, the wire format, or the CRD; adds a validation path with no reachable trigger.
- Treat `Some("")` as a hard error instead of falling through — would convert the safe degrade-to-`secret_ref` behavior into a failure, with no real-world input that exercises it.

## Re-evaluation Triggers

Re-open only if:

- `read_secret_utf8` stops filtering empty/whitespace values (drops the `!value.trim().is_empty()` guard), or a new code path assigns `resolved_*_secret = Some(...)` directly without that filter.
- `resolved_client_secret` / `resolved_session_secret` lose `#[schemars(skip)]` and become operator-settable CRD fields.
- `session_cipher` is changed so an empty secret would be accepted as a valid key (i.e., the fallthrough stops being the safe direction).

## Reference Cases

- F-B03 (Round B adjacent-scope scan, closed 2026-05-22) — `tasks/issues/plugins-http-FB03-oidc-session-secret-some-empty-bypass.md`, closed as accepted-tradeoff with no code change.
- Source: `edgion-gateway/src/plugins/http/openid_connect/session_cookie.rs:11-16,57-62`
- Upstream filter: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/mod.rs:33-45`
