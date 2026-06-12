---
name: challenge-host-split-ipv6-literal
description: The ACME HTTP-01 challenge host extraction now strips the Host port via the IPv6-aware host_without_port helper, preserving brackets so challenge_store.lookup matches the stored entry.domain. Reject re-reports claiming an IPv6-literal Host (e.g. [::1]:80) always yields domain_mismatch.
---

# fr-rhttp-01 — ACME HTTP-01 challenge host port-stripping preserves IPv6-literal brackets

**Status:** ✅ fixed · 2026-05-31
**Severity:** P3 (Correctness) — IPv6-literal Host clients always hit `domain_mismatch`

## Conclusion

`acme_challenge_token_and_host` (`src/core/gateway/routes/http/proxy_http/pg_request_filter.rs`)
now strips the Host-header port via the shared `host_without_port` helper
(`edgion-common/src/utils/net.rs`), which preserves an IPv6 literal's brackets so the extracted
host can match the stored `entry.domain` in `challenge_store.lookup`.

Re-reports of the form "ACME HTTP-01 `Host: [::1]:80` is split into `[`", "the challenge lookup
always returns `domain_mismatch` for an IPv6-literal Host", or "`host.split(':').next()` mangles
IPv6 literals on the ACME challenge path" are **not accepted** — the site no longer uses
`split(':')`.

## What was wrong (before the fix)

The function stripped the port with `host.split(':').next().unwrap_or("")`. For an IPv6-literal
Host such as `[::1]:80`, splitting on the first `:` returns `"["` — a corrupt host that flowed
into `challenge_store.lookup(token, "[")`. The lookup compares `entry.domain == host`
(`acme/challenge_store.rs`), so `"["` never matched any stored domain and the challenge fell
through to `record_acme_challenge_lookup("domain_mismatch")`, silently failing certificate
issuance. Reg-name / IPv4 Hosts were unaffected; only IPv6-literal Host clients hit the broken
branch.

Practically the trigger frequency is near-zero: ACME HTTP-01 validation is performed by the CA
against a **domain name**, so the validating request's `Host` is an FQDN, not a bracketed IPv6
literal. The broken branch only fires in a contrived dev / cluster-internal setup that probes the
challenge path with an IPv6-literal Host. Fixed for consistency with the three sibling sites, not
for a reachable production failure.

## Why the fix is correct

`host_without_port` keeps the brackets for a `[`-prefixed Host (takes up to and including the
closing `]`) and strips a single trailing numeric `:port` for the reg-name / IPv4 branch, with a
`matches(':').count() == 1` guard so a bare/unbracketed IPv6 literal is returned unchanged rather
than mangled. It returns a borrowed `&str`, so the call site uses
`.map(host_without_port).unwrap_or_default().to_string()`. Reg-name / IPv4 Hosts with a numeric
port behave identically to the old `split(':')` code.

Not a security finding: a mangled host can only cause a challenge *miss*, never a cross-domain
*hit* — the token is still validated against `entry.domain`.

## Fix Suggestions Not Accepted

- "Branch on `is_ipv6()` inline at the call site" — duplicates the logic and lets the site drift
  from the three siblings (the original defect was exactly such a duplicated split).
- "Reuse `NormalizedHostname`" — it strips the IPv6 brackets (correct for hostname matching, but
  the challenge lookup compares against `entry.domain`, which is stored with brackets).
- "Raise severity / treat as security" — rejected; the broken branch is unreachable in real ACME
  issuance and can only cause a miss, not a cross-domain hit.

## Re-evaluation Triggers

Re-open only if:

- The ACME challenge host extraction stops going through `host_without_port`.
- The challenge lookup key source changes (e.g. SNI-derived host instead of the raw Host header).

## Reference Cases

- fr-rhttp-01 (review finding, 2026-05-31).
- Same IPv6-authority defect class as
  [redirect-F001](../redirect/host-split-ipv6-literal.md),
  [backends-F001](../backends/ipv6-probe-url-brackets.md),
  [backends-F017](../backends/clusterip-ipv6-no-brackets.md),
  [net-F001](../backends/resolve-domain-ipv6-literal.md), and link-sys-F001. This is a
  *challenge-lookup* site, distinct from redirect-F001's *Location-building* scope.
- Unit tests `acme_challenge_token_and_host_extracts_token_and_strips_host_port` (IPv4/reg-name)
  and `acme_challenge_token_and_host_strips_port_from_ipv6_literal_host` (bracketed IPv6) in
  `src/core/gateway/routes/http/proxy_http/pg_request_filter.rs`; helper coverage in
  `test_host_without_port` (`edgion-common/src/utils/net.rs`).
</content>
</invoke>
