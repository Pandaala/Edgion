# Evaluate OIDC cookie attribute preparation

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `f122dc7a7`.

## Evidence and decision

OpenidConnect::build_cookie_attr_suffix in
`edgion-gateway/src/plugins/http/openid_connect/session_cookie.rs` allocates and
formats an attribute String which is then copied into a final cookie String.
Its session argument is unused. The six production call sites in callback.rs,
request_filter_impl.rs and token_exchange.rs use four path/max-age combinations:
callback path with 300 or zero seconds, and root path with session_lifetime or
zero seconds. Secure, HttpOnly and SameSite derive from generation-owned config.

Choose whether to retain this path, prepare suffixes when constructing the plugin,
or compose the cookie and its attributes directly in one shared formatting owner.
Prepared suffixes avoid repeated formatting but four stored Strings increase
per-generation retained memory, including instances that emit no cookies. Callback
paths also become duplicated text. Direct composition avoids intermediate suffix
storage without persistent prepared Strings, but requires a focused formatting
interface change and exact output validation. Do not introduce four independent
formatters or a global suffix cache.

Emissions occur during authorization, callback, logout and session publication
after refresh; ordinary successful authentication need not emit a cookie. Measure
actual emission workloads and plugin-generation populations rather than assuming
every request pays the suffix cost. No candidate implementation or benchmark has
run, and fewer transient allocations alone do not establish the best strategy.

This is separate from `csrf-01-cookie-header-ownership.md`: that task concerns
transferring a completed cookie into HeaderValue, whereas this task concerns the
producer's intermediate attribute String. Final header ownership is out of scope.

## Semantic boundaries

Preserve exact bytes and order: Path, SameSite, Max-Age, optional HttpOnly, optional
Secure. Keep current case-insensitive Strict/None handling and Lax fallback in the
helper, without silently adding trimming or changing admission. Use the normalized
callback path and configured session lifetime from the same immutable generation.
Preserve Max-Age 300 for authorization state and zero for clearing cookies.

Both state and session cookies continue to use the configured Secure flag, not
transport or forwarded headers. Preserve HttpOnly selection, names, encryption,
nonce generation, persistence/cache ordering, cookie size checks, response ordering,
Set-Cookie append semantics and existing error handling. Preparing metadata must
not move header validation or cookie size rejection to a different response stage.
Do not add token/cookie diagnostics or request-level tracing for measurement.

## Acceptance for a later implementation

Use IMPL with an exact allow-list. Compare full cookie bytes for all four production
combinations, flag combinations, SameSite inputs, callback paths and lifetime
boundaries. Retain distinction between accepted configuration and defensive helper
behavior. Verify generation isolation across config replacement and all existing
callback, logout, refresh, size-limit and multi-cookie response tests.

Compare allocation counts, requested bytes, CPU and retained capacity for each
strategy, including construction and unused/bearer-only instances, long callback
paths, many generations and differing cookie emission rates. Keep crypto and final
HeaderValue ownership outside this measurement change.

Fresh unchanged-code verification passed the two existing cookie-attribute tests.
They check selected substrings and flags, not complete output equivalence for a
future implementation. No benchmark ran. Original authority:
`skills/04-review/oidc/intentional-design/state-cookie-shared-secure-flag.md` and
`skills/04-review/oidc/regression-guards/session-cookie-upstream-ownership.md`.
