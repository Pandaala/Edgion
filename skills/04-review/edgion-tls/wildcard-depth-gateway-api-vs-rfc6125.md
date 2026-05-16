---
name: wildcard-depth-gateway-api-vs-rfc6125
description: Use when reviewing wildcard hostname matching depth findings; explains why hostname_matches_listener (multi-level) and matches_host_pattern (single-level) intentionally follow different standards.
---

# Wildcard Depth: Gateway API vs. RFC 6125 — Not a Code Bug

## Conclusion

The semantic difference between `hostname_matches_listener` (route matching, multi-level)
and `matches_host_pattern` (TLS SAN validation, single-level) is **intentional and correct**.
Both functions implement the standard that governs their respective domain.

- `hostname_matches_listener` — per the Gateway API spec: `*.example.com` matches any
  subdomain depth (`api.example.com`, `a.b.example.com`, …). Correct for HTTP route selection.
- `matches_host_pattern` — RFC 6125 §6.4.3: `*.example.com` matches only a single label
  (`api.example.com`), not `a.b.example.com`. Correct for TLS certificate SAN validation.

These two functions do not call each other and do not produce a code-level conflict.
The operational divergence (route matches but TLS handshake fails for deep subdomains)
comes from **RFC 6125-compliant client TLS libraries** rejecting the handshake — not from
a bug in Edgion's code.

**Fix for g8-006 (2026-05-15)**: rustdoc cross-references added to both functions and
to `TlsCertMatcher::match_sni`; operator-facing notes added to `tls-termination.md`
and `edgion-tls.md`.

## Decision Rule

When a review/audit report claims any of the following, close as "documented tradeoff":

1. "`hostname_matches_listener` and `matches_host_pattern` have inconsistent wildcard semantics — should unify"
2. "Route matching accepts `a.b.example.com` but TLS rejects it — latent bug"
3. "`matches_host_pattern` should use multi-level matching to align with route matching"
4. "`TlsCertMatcher::match_sni` selects a cert but TLS handshake still fails — logic error"
5. "BoringSSL should be configured to accept multi-level wildcard SANs"

Verify before closing:

- `hostname_matches_listener` in `route.rs` still contains the Gateway API §4.3 note.
- `matches_host_pattern` in `validation/mod.rs` still contains the RFC 6125 §6.4.3 note.
- `TlsCertMatcher::match_sni` in `cert_matcher.rs` still contains the BoringSSL note.
- `tls-termination.md` Example 2 still includes the wildcard depth warning.

## Exceptional Scenarios (need reassessment)

- If Gateway API changes its wildcard spec to single-label — reassess `hostname_matches_listener`.
- If BoringSSL adopts multi-level wildcard matching — reassess the BoringSSL note in `match_sni`.
