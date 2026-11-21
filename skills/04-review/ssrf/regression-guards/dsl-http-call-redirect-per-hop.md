---
name: dsl-http-call-redirect-per-hop
description: Use when reviewing DSL `http.call` findings about redirect-bypass of `httpBlockPrivateIps`; explains the literal precheck and dial-time resolved-address guard.
knowledge_type: regression-guards
---

# DSL `http.call` re-checks `block_private_ips` on every redirect hop — FIXED (F-012)

## Conclusion

When `httpBlockPrivateIps: true`, the DSL HTTP client now uses a guarded
`reqwest::Client` whose `redirect::Policy::custom` calls
`is_ssrf_blocked_ip_literal` against each redirect hop's URL host. A redirect
to a blocked IP literal (loopback / private / link-local / ULA / unspecified)
terminates the request with `status = 0` instead of being silently followed.
The 10-hop overall cap is preserved.

With `httpBlockPrivateIps: false` (the default), the client allows private
and local addresses but still checks every redirect literal and resolved address
for non-dialable destinations and configured CIDRs, even with an empty list.

**Do not re-open this as a redirect-bypass SSRF finding.** The first-hop-only
gap that previously allowed `public-host → 302 → 169.254.169.254` was closed
in F-012 (2026-05-26).

## Hostname enforcement

Both private-address policies classify the exact system DNS answers when dialing a hostname.
With private blocking off and no configured CIDRs, the uncached path retains the OS socket's
IPv6 interface scope. All other DSL clients use the request-influenced DNS answer cache.
The resolver classifies the exact DNS results reqwest receives for dialing, so
a redirect to a hostname that resolves to a blocked address fails with
`status = 0`. There is no separate preflight lookup and therefore no
DNS-rebinding TOCTOU window.

## Implementation pointers

- `edgion-gateway/src/plugins/http/dsl/runtime.rs::dsl_http_client(block_private_ips, follow_redirects)`:
  selects among four process-global clients by egress policy and redirect
  behavior. All four clients attach a policy-checking DNS resolver; opaque `Body` clients
  deny redirects.
- `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs::call_builtin_http`:
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
- [dsl-http-call-ip-literal-only.md](../superseded/dsl-http-call-ip-literal-only.md) —
  historical F-018 record, now superseded by dial-time resolution.
- `skills/01-architecture/02-gateway/dsl-engine-security-model.md` §8 —
  maintainer-view description of both guard layers.
