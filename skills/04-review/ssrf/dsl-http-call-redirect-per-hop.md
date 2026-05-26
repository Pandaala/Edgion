---
name: dsl-http-call-redirect-per-hop
description: Use when reviewing DSL `http.call` findings about redirect-bypass of `httpBlockPrivateIps` — explains the per-hop guard that closes F-012 and what is (still) out of scope.
---

# DSL `http.call` re-checks `block_private_ips` on every redirect hop — FIXED (F-012)

## Conclusion

When `httpBlockPrivateIps: true`, the DSL HTTP client now uses a guarded
`reqwest::Client` whose `redirect::Policy::custom` calls
`is_ssrf_blocked_ip_literal` against each redirect hop's URL host. A redirect
to a blocked IP literal (loopback / private / link-local / ULA / unspecified)
terminates the request with `status = 0` instead of being silently followed.
The 10-hop overall cap is preserved.

When `httpBlockPrivateIps: false` (the default), the original permissive
client with `Policy::limited(10)` is used — behavior unchanged.

**Do not re-open this as a redirect-bypass SSRF finding.** The first-hop-only
gap that previously allowed `public-host → 302 → 169.254.169.254` was closed
in F-012 (2026-05-26).

## What's still out of scope (and why)

The same IP-literal-only limitation as the first-hop guard:

- **DNS-name targets** are not resolved before dialing, neither on the first
  hop nor on any redirect hop. A redirect `Location:` pointing at a hostname
  that resolves to a private address is not blocked by this flag. This is the
  same DNS-name limitation documented in
  [dsl-http-call-ip-literal-only.md](dsl-http-call-ip-literal-only.md) and is
  an explicitly accepted tradeoff (security-model §7).
- **IP pinning at connect time** — `reqwest` re-resolves DNS independently;
  no TOCTOU-safe IP pinning has been added.

Findings of the form "redirect to a hostname that resolves privately still
gets through" are the DNS-name tradeoff, not a regression of F-012. Close
them by reference to `dsl-http-call-ip-literal-only.md`.

## Implementation pointers

- `src/core/gateway/plugins/http/dsl/runtime.rs::dsl_http_client(block_private_ips: bool)`:
  selects `DSL_HTTP_CLIENT_GUARDED` (custom policy) when `true`, else
  `DSL_HTTP_CLIENT` (limited policy). Both are process-global so connection
  pools are reused.
- `src/core/gateway/plugins/http/dsl/lang/builtins.rs::call_builtin_http`:
  passes `runtime.block_private_ips` to `dsl_http_client(..)`.
- `is_ssrf_blocked_ip_literal` is `pub(crate)` so the runtime's redirect
  policy can reuse the same predicate — avoid forking the IP-blocklist logic.

## Re-evaluation Triggers

Re-open the per-hop check only if:

- A real SSRF report cites a redirect-hop IP-literal bypass in a production
  deployment.
- The `is_ssrf_blocked_ip_literal` predicate is extended (new CIDR classes)
  but the redirect policy is not updated in lockstep — that would be a real
  regression of F-012.

## Reference Cases

- F-012 (2026-05-26) — `tasks/plugins/plugins-http-dsl-F012-...`, fixed.
- [dsl-http-call-ip-literal-only.md](dsl-http-call-ip-literal-only.md) —
  sibling DNS-name limitation (accepted tradeoff, F-018).
- `skills/01-architecture/02-gateway/dsl-engine-security-model.md` §7 —
  maintainer-view rationale for the IP-literal-only guard.
