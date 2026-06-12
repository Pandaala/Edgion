---
name: host-split-ipv6-literal
description: Both redirect sites now strip the Host port via the IPv6-aware host_without_port helper, preserving brackets in the Location URL. Reject re-reports claiming IPv6-literal Host produces a malformed redirect (https://[/...).
---

# redirect-F001 — Redirect Host port-stripping preserves IPv6-literal brackets

**Status:** ✅ fixed · 2026-05-21
**Severity:** Medium (BadLogic) — IPv6-literal Host clients received a malformed `Location` redirect

## Conclusion

Both redirect call sites now strip the Host-header port via the shared
`host_without_port` helper (`edgion-common/src/utils/net.rs`), which preserves an IPv6
literal's brackets so the host can be re-emitted into a valid URL authority:

- `EdgionHttpRedirectProxy::build_redirect_url_from_parts` (`src/core/gateway/routes/http/redirect_http.rs`)
- `RequestRedirectFilter::build_location` (`src/core/gateway/plugins/runtime/gateway_api_filters/request_redirect.rs`)

Re-reports of the form "redirect builds `https://[/...` for an IPv6 Host", "`[::1]:8443`
Host is split into `[`", or "`host.split(':').next()` mangles IPv6 literals on the redirect
path" are **not accepted** — neither site uses `split(':')` anymore.

## What was wrong (before the fix)

Both sites used `host.split(':').next().unwrap_or(host)`. For an IPv6-literal Host such as
`[::1]:8443`, splitting on the first `:` returns `"["` — a corrupt host that flowed into the
`Location` response header, producing an RFC-3986-invalid redirect target (`https://[/...`).
Reg-name / IPv4 Hosts were unaffected; only IPv6-literal Host clients hit the broken redirect.

## Why the fix is correct

`host_without_port` mirrors the existing `NormalizedHostname` split logic (`rfind(':')` +
digit-suffix check for the reg-name/IPv4 branch; `[`-prefixed → take up to and including the
closing `]`) but **keeps the brackets** and does not lowercase or strip a trailing FQDN dot.
`NormalizedHostname` itself could not be reused: it strips the brackets (`[::1]:8443` → `::1`)
because it is built for hostname *matching*, whereas re-emitting `::1` without brackets into a
URL would again be malformed. The helper returns a borrowed `&str` slice (no allocation), so it
is a drop-in replacement at both call sites. Reg-name / IPv4 Hosts with a numeric port behave
identically to the old `split(':')` code; the only non-IPv6 difference is that a Host with a
*non-numeric* `:suffix` (e.g. `host:notaport`) is now preserved rather than truncated — the
digit-only guard treats it as not-a-port, which is more correct.

The reg-name/IPv4 branch additionally guards with `host.matches(':').count() == 1`, so a
**bare/unbracketed** IPv6 literal (`2001:db8::1`, `::1` — >1 colon, no strippable port) is
returned unchanged instead of being mangled into `2001:db8:`. Realistic callers only ever
receive a *bracketed* IPv6 Host per RFC 7230/3986, so this guards a shared-helper footgun
rather than a reachable redirect bug (hardened during deep review 2026-05-21).

## Fix Suggestions Not Accepted

- "Use `http::uri::Authority` to parse the Host" — workable, but introduces a parse-fail
  fallback branch and a new parsing pattern in this area; the hand-rolled helper matches the
  existing `NormalizedHostname` style, is dependency-free, and has no parse-fail edge case.
- "Reuse `NormalizedHostname` directly" — it strips the IPv6 brackets (correct for matching,
  wrong for re-emitting into a URL).
- "Branch on `is_ipv6()` inline at each call site" — duplicates the logic and lets the two
  sites drift again (the original defect was exactly such a duplicated split).

## Re-evaluation Triggers

Re-open only if:

- A third redirect/Location-building site is added that strips the Host port without going
  through `host_without_port`.
- The Host source stops being the raw header string (e.g., a pre-normalized value that already
  dropped the brackets is fed into a `Location` builder).

## Reference Cases

- redirect-F001 (review finding, deep review 2026-05-20; fixed 2026-05-21)
- Same IPv6-authority defect class as [backends-F001](../backends/ipv6-probe-url-brackets.md),
  [backends-F017](../backends/clusterip-ipv6-no-brackets.md), and link-sys-F001.
- Unit test `test_host_without_port` (`edgion-common/src/utils/net.rs`); integration regression
  `http_redirect_ipv6_host_literal` (`examples/code/client/suites/http_route/filters/redirect.rs`)
  sends `Host: [::1]:8443` to the global HTTP→HTTPS redirect listener and asserts the `Location`
  keeps the brackets.
