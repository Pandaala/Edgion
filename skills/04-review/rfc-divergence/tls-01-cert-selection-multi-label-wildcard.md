---
name: tls-01-cert-selection-multi-label-wildcard
description: Accepted RFC 6125 §6.4.3 divergence (deferred fix) — HashHost-based TLS cert selection currently multi-label-matches `*.example.com` against `a.b.example.com`. Identified as a gap, not permanent design. Reject re-reports as new findings; point to tasks/todo/tls-cert-selection-single-label-rfc6125.md.
---

# tls-01 — HashHost wildcard cert selection accepts deeper SNI than RFC 6125 §6.4.3

**Status:** ⚠️ accepted divergence (deferred fix) · 2026-05-19
**RFC:** RFC 6125 §6.4.3 (SHOULD NOT — client-side constraint), RFC 5280 §4.2.1.6 (MAY — server-side leeway)

## Conclusion

Edgion's TLS certificate selection by SNI uses the general-purpose `HashHost` matcher,
which performs **multi-level** wildcard suffix matching. A certificate registered for
`*.example.com` is therefore returned when a client sends SNI `a.b.example.com`.

RFC 6125 §6.4.3 restricts the wildcard to **exactly one** left-most label; an
RFC-6125-compliant client TLS library aborts the handshake with `bad_certificate`
when it validates the served SAN against its requested SNI.

**This divergence is identified as a real gap, not a permanent design choice.** The
project will tighten cert selection to single-label semantics in a future TLS
maintenance window. The deferred work is tracked at
[`tasks/todo/tls-cert-selection-single-label-rfc6125.md`](../../../tasks/todo/tls-cert-selection-single-label-rfc6125.md).

Until then, re-reports of the same pattern (different verifier, different sweep) **must
not** be raised as new findings — they should be closed by reference to this entry and
to the deferred task.

## RFC clauses

> "If the wildcard character is the only character of the left-most label in the
> presented identifier, the client SHOULD NOT compare against more than one label
> of the reference identifier (e.g., `*.example.com` matches `foo.example.com` but
> not `bar.foo.example.com`)." — RFC 6125 §6.4.3

> "An asterisk (`*`) MAY be used as the left-most name component … `*.example.com`
> would match `a.example.com`, `foo.example.com`, etc., but would not match
> `example.com`." — RFC 5280 §4.2.1.6

Strictness: client-side **SHOULD NOT**; server-side (which is where Edgion sits) has
no MUST in the RFCs. The reason this is still worth fixing is **interoperability**:
modern client TLS stacks (BoringSSL, recent OpenSSL, Go, Node, browsers) implement
the §6.4.3 single-label rule and will reject Edgion's served cert.

## Code shape today

- `src/core/common/matcher/host_match/hash_match/hash_host.rs:86-94` —
  `HashHost::get_lower` does suffix-length lookup; `*.example.com` is stored as
  `.example.com` (12 chars); query `a.b.example.com` (15 chars) satisfies
  `k_len > wk_len` and the 12-char suffix slice equals `.example.com` → hit.
- `src/core/gateway/tls/store/cert_matcher.rs:51-62` —
  `TlsCertMatcher::match_sni` uses `host_matcher.get(sni)` → multi-label hit.
  Doc-comment at lines 47-50 already acknowledges the §6.4.3 mismatch.
- `src/core/gateway/runtime/matching/tls.rs:98-119` —
  `GatewayTlsMatcher::match_sni` (Gateway-API native cert selection fallback)
  uses the same `host_matcher.get(sni)` path.
- `src/core/gateway/routes/tls/gateway_tls_routes.rs:89-105` —
  `TlsRouteTable::match_route` ALSO uses `HashHost::get`, but this is **route
  matching** (TLSRoute selection) where multi-level wildcards are correct per
  Gateway API §4.3. This call site is intentionally NOT a divergence and must
  stay multi-label after the fix.
- `src/core/gateway/tls/validation/mod.rs::matches_host_pattern` — uses the
  RFC-6125-strict single-label semantics; used for SAN-vs-listener validation
  at config-load time. The deferred fix in cert selection brings the runtime
  selection path in line with this already-correct validation path.

## Why we (currently) diverge

1. **Infrastructure reuse, not a design choice.** `HashHost` was introduced in
   2026-04-06 (commit 46f9ba3, "support Gateway API conformance1.5") for
   HTTPRoute / TLSRoute matching where Gateway API §4.3 requires multi-level
   wildcards. The TLS cert selection path subsequently reused it for SNI
   lookups; the multi-level semantics carried over implicitly. No commit
   message records "we chose multi-label for cert selection on purpose."
2. **Operator-visible behavior regression deserves a deprecation window.**
   Switching to single-label means deployments that today happen to handshake
   successfully against lenient clients will fail closed. The change is small
   (~30 LoC), but a release-notes line and ops-guide update are required;
   bundling those with the next TLS maintenance touch is cheaper than a
   stand-alone PR.
3. **Industry reference (Envoy) is consistent with the deferred direction.**
   `source/common/tls/default_tls_certificate_selector.cc::findTlsContext`
   strips exactly one left-most label and does not loop; the test
   `test/common/tls/ssl_socket_test.cc::MultiCertWildcardSniMismatch` asserts
   that `*.example.com` is NOT served for SNI `a.wildcardonlymatch.example.com`.
   Envoy's filter-chain `server_names` layer (the analogue of Edgion's
   `TlsRouteTable`) is multi-label, mirroring Edgion's intended end state.

## Future work / unblocking conditions

Pick this up when any of the following is true:
- The TLS subsystem is being touched for another reason (multi-cert RSA+ECDSA,
  cert hot-reload, ACME revisions).
- An operator reports the SNI mismatch issue on a production deployment.
- A Gateway API conformance update arrives that touches cert selection.

Migration spec, scoped diffs, doc rewrites, and integration-test outline are in
[`tasks/todo/tls-cert-selection-single-label-rfc6125.md`](../../../tasks/todo/tls-cert-selection-single-label-rfc6125.md).
On fix, this rfc-divergence file should be deleted (or moved to a "resolved"
archive) and `wildcard-depth-gateway-api-vs-rfc6125.md` updated to delete its
Decision Rule #4.

## Reference

- Verifier: `tasks/todo/rfc-review/_raw/verification-a4.md → V02` (CONFIRM)
- Closure: [CLOSED-FINDINGS.md](../CLOSED-FINDINGS.md) → tls-01
- Related skill: [`edgion-tls/wildcard-depth-gateway-api-vs-rfc6125.md`](../edgion-tls/wildcard-depth-gateway-api-vs-rfc6125.md) — explains why `hostname_matches_listener` (multi-level) and `matches_host_pattern` (single-level) coexist intentionally; this entry adds the third layer (`HashHost` at cert selection) as a deferred convergence target.
- Envoy: `envoyproxy/envoy` source `source/common/tls/default_tls_certificate_selector.cc::findTlsContext`; test `test/common/tls/ssl_socket_test.cc::MultiCertWildcardSniMismatch`.
- Related issues: rfc-violation-tls-05 (multi-star wildcard inconsistency) — separate code path; rfc-doc-comments-additions.md `dc-tls-13` — adds RFC citation to `matches_host_pattern`.
