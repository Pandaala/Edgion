---
name: oidc-send-error-extract-function
description: Use when reviewing openid_connect "handle_callback repeats the 502-vs-401 error dispatch N times / extract a send_oidc_error helper / collapse the if http_status()==502 {plain} else {unauth} blocks" findings (F-010). The leaf helper is already extracted; the request_filter_impl.rs:187 fall-through site must NOT be folded into it.
---

# OIDC `send_oidc_error` leaf extraction — FIXED

**Audit date**: 2026-05-26
**Linked issue**: plugins-http F-010 (extract-function / readability) — closed, fixed.

## Conclusion

The repeated `if err.http_status() == 502 { send_plain_error(502) } else { send_unauthorized() }`
dispatch in the OIDC plugin is now centralized in
`OpenidConnect::send_oidc_error(&self, session, err: &OidcVerifyError)`
(`edgion-gateway/src/plugins/http/openid_connect/callback.rs`, next to `send_plain_error` /
`send_unauthorized`). The six identical call sites — five in `callback.rs::handle_callback`
(decode state cookie, resolve redirect_uri, token exchange, validate nonce, verify_token)
and one in `request_filter_impl.rs` (the `UnauthAction::Auth` build-redirect arm) — now call
the helper. The caller still owns `plugin_log.push_err(...)` and the returned
`PluginRunningResult`; only the status-to-response decision moved into the leaf.

Re-files of the form "extract a `send_oidc_error` helper / collapse the seven 502-vs-401
blocks" are moot — verify the helper exists before re-reporting.

## Core Rationale

**1. This is the accepted *leaf* kind of extraction, not the declined control-flow kind**

`send_oidc_error` carries no control flow: the `return PluginRunningResult::ErrTerminateRequest`
and the per-site `push_err` stay at the call sites. That is the same low-risk, drift-prevention
leaf extraction the project *accepted* in link-sys (`credential_readiness`), as opposed to the
`ControlFlow` / `LifecycleAction` re-encodings it *declined* in
[link-sys-dispatch-extract-function.md](link-sys-dispatch-extract-function.md) and
[webhook-call-extract-function.md](webhook-call-extract-function.md).
It centralizes a security-relevant 502-vs-401 decision on the OIDC auth path, satisfying the
"drift-prevention" exception in [style-consolidation-thresholds.md](style-consolidation-thresholds.md).

**2. `request_filter_impl.rs:187` is a DIFFERENT shape and is intentionally NOT folded in**

The token-refresh/verify error arm at `request_filter_impl.rs:187` has ONLY the 502 branch
(`if err.http_status() == 502 { send_plain_error; return }`), with NO `else`: a non-502 (auth)
error there deliberately falls through to the `unauth_action` match below. Calling
`send_oidc_error` (which always sends one of the two) at that site would send a spurious 401
and break the fall-through — an observable behavior regression. The original finding miscounted
this as the 7th occurrence of the same pattern; it is not.

## Fix Suggestions Not Accepted

- "Also route `request_filter_impl.rs:187` through `send_oidc_error` to unify all seven sites" — that site has no `else` branch and falls through on non-502; folding it in changes behavior. Leave it as-is.
- "Move `return ErrTerminateRequest` / `push_err` into the helper too" — that re-encodes per-site control flow and per-site log tags behind the helper; the leaf intentionally stops at the response dispatch (same posture as the declined link-sys control-flow unifications).

## Re-evaluation Triggers

Re-open only if:

- A real bug is traced to the 502-vs-401 decision drifting between a call site and the helper.
- `request_filter_impl.rs:187` gains an `else` arm that makes it genuinely identical to the other six — only then fold it in.

## Reference Cases

- plugins-http F-010 (`tasks/plugins/plugins-http-F010-openid-connect-multi-file-extract-fn.md`), fixed 2026-05-26.
- `edgion-gateway/src/plugins/http/openid_connect/callback.rs` — `send_oidc_error` and its five call sites.
- `edgion-gateway/src/plugins/http/openid_connect/request_filter_impl.rs` — the `UnauthAction::Auth` call site (folded) vs the `:187` fall-through site (untouched).
- Sibling leaf-vs-control-flow extraction decisions: [link-sys-dispatch-extract-function.md](link-sys-dispatch-extract-function.md), [webhook-call-extract-function.md](webhook-call-extract-function.md), [style-consolidation-thresholds.md](style-consolidation-thresholds.md).
