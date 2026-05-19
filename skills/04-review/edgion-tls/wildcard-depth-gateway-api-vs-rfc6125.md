---
name: wildcard-depth-gateway-api-vs-rfc6125
description: Use when reviewing wildcard hostname matching depth findings; explains why hostname_matches_listener (multi-level, Gateway API) and matches_host_pattern (single-level, RFC 6125) intentionally follow different standards, and why HashHost at the TLS cert-selection layer is a known-deferred convergence target (see tls-01).
---

# Wildcard Depth: Three Layers, Two Standards

## Conclusion

Edgion has **three** wildcard-hostname code paths. Two are correctly aligned with
the standards that govern their domains; the third is a known gap with a
deferred fix.

| Layer | Code | Wildcard depth | Governing standard | Status |
|-------|------|----------------|--------------------|--------|
| HTTP / TLS route matching | `route::hostname_matches_listener` | multi-level | Gateway API §4.3 | ✅ correct |
| TLS cert SAN ↔ listener config validation | `tls::validation::matches_host_pattern` | single-level | RFC 6125 §6.4.3 | ✅ correct |
| TLS cert selection by SNI at handshake time | `HashHost::get` used by `TlsCertMatcher::match_sni`, `GatewayTlsMatcher::match_sni` | multi-level | should be RFC 6125 §6.4.3 (single-level) | ⚠️ deferred (see [tls-01](../rfc-divergence/tls-01-cert-selection-multi-label-wildcard.md)) |

The first two diverge intentionally — they govern different domains:

- `hostname_matches_listener` — per the Gateway API spec: `*.example.com` matches
  any subdomain depth (`api.example.com`, `a.b.example.com`, …). Correct for
  HTTPRoute / TLSRoute selection.
- `matches_host_pattern` — per RFC 6125 §6.4.3: `*.example.com` matches only a
  single label (`api.example.com`), not `a.b.example.com`. Correct for TLS
  certificate SAN validation at config load time.

These two functions do not call each other and do not produce a code-level
conflict. The operational divergence (route matches but TLS handshake fails for
deep subdomains) comes from **RFC 6125-compliant client TLS libraries** rejecting
the handshake — not from a bug in Edgion's code.

The third path — `HashHost::get` used at TLS cert **selection** by SNI — currently
inherits the multi-level semantics from its general-purpose matcher origin. This
is **not** an intentional design choice; the cert-selection layer should be
single-label per RFC 6125, matching what `matches_host_pattern` already enforces
at config load and what Envoy does at the same layer
(`source/common/tls/default_tls_certificate_selector.cc::findTlsContext`,
single-strip, no loop). The fix is tracked at
[`tasks/todo/tls-cert-selection-single-label-rfc6125.md`](../../../tasks/todo/tls-cert-selection-single-label-rfc6125.md);
the divergence record is at
[`rfc-divergence/tls-01-cert-selection-multi-label-wildcard.md`](../rfc-divergence/tls-01-cert-selection-multi-label-wildcard.md).

**Original closure (g8-006, 2026-05-15)**: rustdoc cross-references added to both
`hostname_matches_listener` and `matches_host_pattern`, plus an acknowledgement
note on `TlsCertMatcher::match_sni`; operator-facing notes added to
`tls-termination.md` and `edgion-tls.md`. The cert-selection acknowledgement was
later (2026-05-19) escalated from "documented tradeoff" to "deferred fix" after
the RFC review (tls-01) re-examined the same code path with a sharper eye on
Envoy's precedent and on the existing `matches_host_pattern` single-label
implementation already living in the same project.

## Decision Rule

When a review/audit report claims any of the following, close as "documented tradeoff":

1. "`hostname_matches_listener` and `matches_host_pattern` have inconsistent
   wildcard semantics — should unify" → still **rejected**; intentional.
2. "Route matching accepts `a.b.example.com` but TLS rejects it — latent bug"
   → still **rejected**; the route layer is correct per Gateway API, the
   client-side TLS rejection is per RFC 6125. The two are independent.
3. "`matches_host_pattern` should use multi-level matching to align with route
   matching" → still **rejected**; would silently weaken cert SAN validation.
4. "`TlsCertMatcher::match_sni` / `GatewayTlsMatcher::match_sni` selects a cert
   but TLS handshake still fails — logic error":
   - **Do not raise as a new finding.** Point to
     [`rfc-divergence/tls-01-cert-selection-multi-label-wildcard.md`](../rfc-divergence/tls-01-cert-selection-multi-label-wildcard.md)
     and [`tasks/todo/tls-cert-selection-single-label-rfc6125.md`](../../../tasks/todo/tls-cert-selection-single-label-rfc6125.md).
   - Re-evaluation is welcome when the deferred task is picked up; until then,
     the gap is known and tracked.
5. "BoringSSL should be configured to accept multi-level wildcard SANs"
   → **rejected**; BoringSSL implements the same RFC 6125 §6.4.3 rule and is
   correct. The fix lives in Edgion's cert-selection path, not in BoringSSL.

Verify before closing:

- `hostname_matches_listener` in `route.rs` still contains the Gateway API §4.3 note.
- `matches_host_pattern` in `validation/mod.rs` still contains the RFC 6125 §6.4.3 note.
- `TlsCertMatcher::match_sni` in `cert_matcher.rs` still contains the BoringSSL note.
- `tls-termination.md` Example 2 still includes the wildcard depth warning.
- `tasks/todo/tls-cert-selection-single-label-rfc6125.md` still exists; if it has
  been deleted, the fix has landed and this skill (specifically rule 4) should
  be retired or rewritten.

## Exceptional Scenarios (need reassessment)

- If Gateway API changes its wildcard spec to single-label — reassess
  `hostname_matches_listener`.
- If BoringSSL adopts multi-level wildcard matching — reassess the BoringSSL
  note in `match_sni`.
- If the deferred fix in
  `tasks/todo/tls-cert-selection-single-label-rfc6125.md` lands — delete this
  skill's Decision Rule #4, delete
  `rfc-divergence/tls-01-cert-selection-multi-label-wildcard.md`, and rewrite
  `cert_matcher.rs`'s doc-comment to describe the new single-label behavior.
