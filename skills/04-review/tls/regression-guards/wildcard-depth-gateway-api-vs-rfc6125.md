---
name: wildcard-depth-gateway-api-vs-rfc6125
description: Use when reviewing wildcard hostname matching depth findings; explains why Route hostname matching is multi-level while RFC 6125 validation and certificate selection are single-level.
knowledge_type: regression-guards
---

# Wildcard Depth: Three Layers, Two Standards

## Conclusion

Edgion has **three** wildcard-hostname code paths. All three are correctly aligned with
the standards that govern their domains.

| Layer | Code | Wildcard depth | Governing standard | Status |
|-------|------|----------------|--------------------|--------|
| HTTP / TLS route matching | `route::hostname_matches_listener` | multi-level | Gateway API §4.3 | ✅ correct |
| TLS cert SAN ↔ listener config validation | `edgion_algo::matcher::rfc6125_hostname::matches` (+ `is_valid_san_pattern` pre-pass) | single-level | RFC 6125 §6.4.3 | ✅ correct |
| TLS cert selection by SNI at handshake time | `edgion_algo::matcher::rfc6125_hostname::lookup`, called by persistent `TlsCertMatcher` slots and `Rfc6125HostnameMatcher::get` for Gateway TLS | single-level | RFC 6125 §6.4.3 | ✅ correct |

The first two diverge intentionally — they govern different domains:

- `hostname_matches_listener` — per the Gateway API spec: `*.example.com` matches
  any subdomain depth (`api.example.com`, `a.b.example.com`, …). Correct for
  HTTPRoute / TLSRoute selection.
- `rfc6125_hostname::matches` — per RFC 6125 §6.4.3: `*.example.com` matches only a
  single label (`api.example.com`), not `a.b.example.com`. Correct for TLS
  certificate SAN validation at config load time.

These two functions do not call each other and do not produce a code-level
conflict. The operational divergence (route matches but TLS handshake fails for
deep subdomains) comes from **RFC 6125-compliant client TLS libraries** rejecting
the handshake — not from a bug in Edgion's code.

The third path — `Rfc6125HostnameMatcher::get` used at TLS cert **selection** by SNI — also
diverges intentionally from the route layer: it follows RFC 6125 §6.4.3 (single-label)
to align with what RFC 6125-compliant client TLS stacks expect, and with what
`rfc6125_hostname::matches` already enforces at config load time. This convergence was
completed on 2026-05-19; the previous multi-label behavior is no longer reachable.
Envoy's `source/common/tls/default_tls_certificate_selector.cc::findTlsContext`
(single-strip, no loop) is the reference behavior at this layer.

**Original closure (g8-006, 2026-05-15)**: rustdoc cross-references added to both
`hostname_matches_listener` and `rfc6125_hostname::matches`, plus an acknowledgement
note on `TlsCertMatcher::match_sni`; operator-facing notes added to
`tls-termination.md` and `edgion-tls.md`. The cert-selection convergence was
completed shortly after on 2026-05-19 via `Rfc6125HostnameMatcher::get`.

## Decision Rule

When a review/audit report claims any of the following, close as "documented tradeoff":

1. "`hostname_matches_listener` and `rfc6125_hostname::matches` have inconsistent
   wildcard semantics — should unify" → still **rejected**; intentional.
2. "Route matching accepts `a.b.example.com` but TLS rejects it — latent bug"
   → still **rejected**; the route layer is correct per Gateway API, the
   client-side TLS rejection is per RFC 6125. The two are independent.
3. "`rfc6125_hostname::matches` should use multi-level matching to align with route
   matching" → still **rejected**; would silently weaken cert SAN validation.
4. "`TlsCertMatcher::match_sni` / `GatewayTlsMatcher::match_sni` returns a cert
   for a multi-label SNI like `a.b.example.com` against `*.example.com`":
   - **Now a real bug — raise it.** As of 2026-05-19, cert selection uses
     `Rfc6125HostnameMatcher::get` (single-label per RFC 6125 §6.4.3). Multi-label
     matches at this layer are a regression and should be filed as findings.
5. "BoringSSL should be configured to accept multi-level wildcard SANs"
   → **rejected**; BoringSSL implements the same RFC 6125 §6.4.3 rule and is
   correct. The fix lives in Edgion's cert-selection path, not in BoringSSL.
6. "Validator and matcher disagree on multi-star patterns; a declared host like
   `*.*.example.com` yields a confusing `SanMismatch`" (tls-05)
   → **resolved 2026-05-20.** `validate_cert` now runs an `is_valid_san_pattern`
   pre-pass (`validation/cert.rs`) that emits a dedicated
   `CertValidationError::InvalidSanPattern { pattern }` for any multi-label,
   partial-label, or non-leftmost wildcard, instead of a generic `SanMismatch`.
   The runtime SNI matcher already rejects these via `Rfc6125HostnameMatcher::get`
   (single-label). The original resolution left the disconnected permissive
   `EdgionTls::matches_hostname` / `wildcard_match` helpers untouched. They have
   since been removed together with their 14 helper-only tests after confirming
   that no production caller remained; the historical `get_tls_by_host` caller
   was also already absent. Canonical Algo RFC 6125 matching tests and Common
   `InvalidSanPattern` tests retain the live contract. This removal changes no
   certificate-selection or validation behavior. Re-reports must verify the
   current live owners before claiming a validator/matcher inconsistency.

Verify before closing:

- `hostname_matches_listener` in `route.rs` still contains the Gateway API §4.3 note.
- `rfc6125_hostname::matches` in `edgion-algo/src/matcher/rfc6125_hostname/mod.rs` still enforces the RFC 6125 §6.4.3 rule.
- `TlsCertMatcher::match_sni` in `cert_matcher.rs` still contains the RFC 6125 §6.4.3 + Gateway API §4.3 cross-reference note.
- `tls-termination.md` Example 2 still includes the wildcard depth warning.

## Exceptional Scenarios (need reassessment)

- If Gateway API changes its wildcard spec to single-label — reassess
  `hostname_matches_listener`.
- If BoringSSL adopts multi-level wildcard matching — reassess the BoringSSL
  note in `match_sni`.
- If a future Gateway API conformance suite asserts multi-label cert selection
  (unlikely — would contradict RFC 6125) — reassess whether to add an opt-in
  legacy mode to `Rfc6125HostnameMatcher::get`.

## Resolution references

- `tls-01` — 2026-05-19 (resolved).
- `tls-05` — 2026-05-20.
