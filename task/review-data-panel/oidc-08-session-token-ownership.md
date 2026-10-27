# Evaluate OIDC session token ownership across refresh

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `ac2bc6b75`.

## Evidence and decision

`openid_connect/token_exchange.rs::resolve_access_token_from_session` copies a
nonblank cookie access token into a request-owned String. Otherwise it obtains
an independently owned token from the access-token cache. The request filter
retains this selected token through verification, optional userinfo and upstream
header publication. Refresh helpers mutate the session cookie and return another
copy of its access token; the singleflight result cache separately owns a cloned
cookie with a five-second validity window. That window is not a destruction or
secret-erasure guarantee.

Decide whether the combined request/session representation should borrow cookie
tokens, retain owned cache results, and acquire ownership only when refresh needs
it. A Cow return alone conflicts with subsequent mutable cookie access. The
request filter's cache-miss, preemptive and expired-token branches need explicit
ownership transitions, including retaining the old selected token after failed
refresh. Moving a token out of the cookie also changes what persistence and
refresh-result caching observe unless the owner is restored correctly.

This is not evidence that every copy is necessary. A narrower refresh API that
returns unit now lets the expired-refresh success branch borrow the updated
cookie token. This independent optimization does not decide the combined
representation: the cache-miss and preemptive callers still clone the updated
token, relocating their copies rather than removing them. The evidence above
describes the inspected baseline before this narrower change.

Keep the current combined representation if measured savings do not justify
the branch and ownership complexity. No replacement has been implemented or
benchmarked. This differs from [cookie serialization projection](../../skills/04-review/oidc/regression-guards/session-cookie-upstream-ownership.md#session-persistence-projection-ownership-oidc-07)
and [introspection claims ownership](oidc-05-introspection-claims-ownership.md).

## Boundaries and acceptance

Preserve cookie-token precedence over cache lookup and the existing distinction
between trim-based resolution/preemptive checks and empty-string request checks.
Keep the exact selected token bytes, including surrounding whitespace. Preserve
cache TTL, independent result-cache ownership, singleflight key/lock behavior,
session-reference generation before potentially failing refresh, token rotation,
optional ID/refresh fields, and cookie/cache publication order.

A failed preemptive refresh must still permit verification of the old selected
token. Only the expired-token refresh wait-timeout path may use the existing
allow-expired verifier plus leeway check. Successful refresh must verify and
publish the returned generation's token. Preserve failure routing, redaction,
Gateway-cookie stripping and best-effort userinfo. Do not add caches, tracing,
metrics, compatibility shims or a new credential-forwarding policy.

For a later implementation, cover cookie and cache sources, absent/empty/blank
tokens, cache misses, preemptive success/failure, expired refresh success/failure,
timeout with/without a recent cached result, and exact published token identity.
Measure allocations, copied bytes and retained storage across representative
branches; distinguish local copy savings from whole-request throughput.

## Current verification

The unchanged default OIDC suite passed 162 tests with 16 existing ignores.
Existing coverage includes cache resolution and both singleflight wait-timeout
outcomes. The local-IdP failed-preemptive-refresh fixture remains ignored and was
not run; the full ownership-transition matrix above is future acceptance work.
Formatting and workspace all-target check/Clippy passed with existing warnings.
That documentation batch delivered no runtime implementation, full suite, live
IdP test or benchmark. The later narrow refresh-result optimization is recorded
in INDEX; the combined representation remains deferred.
