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

## CSRF Cookie producer ownership (csrf-01)

The current Cookie Display to String to canonical string append path is retained
as an evaluated tradeoff, not a correctness defect. With locked cookie 0.18.2,
http 1.5.0 and bytes 1.12.1, HeaderValue::try_from(String) transfers storage;
from_maybe_shared(String) instead copies. Direct transfer can retain Display's
spare capacity and allocate Shared immediately; compact copied storage can defer
that allocation until its first clone.

An isolated optimized probe compared complete Display, conversion and drop across
72 valid fixture/lifecycle combinations, including first/repeated clones and a
move/drain. Invalid bypass fixtures matched conversion acceptance; the probe did
not simulate adapter binding or establish accepted configuration. Direct transfer
was faster in these local medians, but retained approximately 1.60–1.95 times the
formatted payload length. For the 17-byte default-name fixture, Secure enabled and
Max-Age 3600, output length was 298 versus Display capacity 508. Direct consumption
measured five allocation events for all three alternatives: old copy requested
1111 bytes, transfer 837, and compact transfer 1111; timing medians were about
204, 176 and 184 ns respectively. The first clone added one 24-byte allocation to
old/compact storage, versus none for already-shared transferred storage. These are
synthetic host/allocator measurements, not retained RSS or whole-plugin throughput.

Retaining compact queued storage and the existing canonical failure policy is
preferred over this local timing gain. The string adapter deliberately returns a
successful no-op for malformed values without a bound response and an error when
bound. Early typed conversion in CSRF cannot reproduce that distinction through
its current session API. A new generic owned-string seam or manual Cookie formatter
is not justified solely for this cookie. No token generation, cookie bytes,
attribute order, Secure/HttpOnly policy, append ordering or PluginLog outcome changes.

Re-evaluate if a separately justified canonical owned session boundary preserves
these outcomes, formatting capacity changes, or representative complete-lifecycle
workloads alter the tradeoff. This decision does not prohibit owned header transfer
for other producers with different storage and validation boundaries.
