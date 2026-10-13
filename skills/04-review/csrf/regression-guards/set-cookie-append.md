---
name: set-cookie-append
description: Read when a finding claims a plugin queues Set-Cookie via set_response_header so a CSRF / OIDC / DSL ctx.set cookie wipes upstream Set-Cookie values, that PendingHeaderOp::Set::apply calls insert_header (replace-all) for Set-Cookie, or that two cookie-writing plugins overwrite each other.
knowledge_type: regression-guards
---

# Plugins queue Set-Cookie via set_response_header, wiping other Set-Cookie values (gateway-plugins-02)

**Status / verdict**: fixed (gateway-plugins-02, 2026-05-24)

When reviewing findings of the form "plugin queues `Set-Cookie` via `set_response_header` / CSRF (or OIDC, or DSL `ctx.set` cookie) cookie wipes all upstream Set-Cookie values / `PendingHeaderOp::Set::apply` calls `insert_header` (replace-all) for Set-Cookie / two cookie-writing plugins overwrite each other": **fixed (gateway-plugins-02, 2026-05-24)**. The response-side sibling of F-001. All four Set-Cookie writers now call `append_response_header` (→ `PendingHeaderOp::Append` → pingora `append_header`) instead of `set_response_header` (→ `insert_header`, which replaces every same-name value): `csrf/plugin.rs::set_csrf_cookie`, `openid_connect/token_exchange.rs::maybe_set_session_cookie_header`, and both `session_adapter.rs::key_set` `KeySet::Cookie` branches (set + Max-Age=0 delete). Set-Cookie is a multi-value header that must NOT be coalesced (RFC 6265 §3 / RFC 7230 §3.2.2 list-syntax exception; Envoy keeps it out of its O(1) inline-header set for the same reason). Name-level dedup is intentionally NOT done — the browser handles same-name last-wins per RFC 6265 §5.4, and the queue drains AFTER the upstream header (`plugin_runtime.rs` `run_upstream_response_plugins_sync`), so a gateway cookie that collides with an upstream cookie name deterministically wins. The issue's suggested defense-in-depth guard in `PendingHeaderOp::Set::apply` (reroute Set-Cookie to append) is **not accepted** — masks the call-site bug instead of fixing the origin. Integration regression: `upstream_set_cookies_preserved` in `edgion-tests/src/client/suites/edgion_plugins/csrf/csrf.rs` (upstream `/multi-set-cookie` returns `upstream_a` + `upstream_b`, CSRF appends its token, asserts all three survive via `get_all`). Re-reports are not accepted; verify the four writers use `append_response_header` before re-reporting.


## Integration failure diagnostics (ARCH-349)

The static CSRF suite reports cookie presence and counts, never raw Set-Cookie
values, when cookie preservation fails. Safe-method issuance and token-extraction
failures also omit cookie contents. Values remain local where token extraction
and subsequent signed-token requests need them. Existing `get_all` multiplicity
and the three-cookie success predicate are unchanged.

A pure verdict regression includes a distinctive issued-token marker with one
missing upstream cookie and verifies failure, useful presence/count context,
and marker absence from reporter-bound error/message data. Complete and
missing-CSRF controls retain the verdict boundary. This verifies diagnostic
safety, not a fresh live append, signature or dynamic-rotation run.
