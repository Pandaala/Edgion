---
name: subdomain-wildcard-https-only
description: Use when reviewing CORS allow_origins wildcard shorthand findings; documents that *.domain matches HTTPS only (by design) and why reverting to https?:// is not accepted.
---

# CORS `*.domain` Shorthand Matches HTTPS Only (Fixed)

## Conclusion

The `*.example.com` shorthand of `CorsConfig.allow_origins` **matches only `https://` origins**. This is the explicit semantic after the 2026-04-21 fix to M13 — earlier versions allowed `http://`, an unintentional behavior of Edgion's bespoke convenience syntactic sugar. The shorthand **should not** be reverted to `https?://` to "be compatible with http" — operators needing http matching should explicitly use `allow_origins_by_regex`.

## Reasons

1. **`*.domain` shorthand is Edgion's bespoke convenience syntactic sugar**, not industry convention (Traefik / Envoy / Kong all let the operator write the regex); since it is a syntactic sugar Edgion defined itself, the semantics should match the operator's mental model
2. **Operator intuition is HTTPS**: nearly all modern CORS production configs target https frontends; the default expectation when writing `*.example.com` is the https scope
3. **Allowing the http scheme expands the attack surface**:
   - Subdomain takeover: an unclaimed subdomain (e.g., `dev-api.example.com` pointing to a decommissioned service) is filled by an attacker with an HTTP service → cross-origin reads of API responses
   - Legacy http subdomains: enterprise intranets often have test/legacy environments exposed over http; would be unintentionally included by the operator in the CORS allowlist
4. **Explicit regex as escape hatch**: scenarios needing http matching (local dev, legacy internal services) can still use `allow_origins_by_regex: ["^https?://[^/]+\\.example\\.com$"]` to declare explicitly
5. **Browser security trends**: `SameSite=None` requires `Secure`; mixed content restrictions; COOP/COEP requires secure context — the industry direction is HTTPS-only CORS

## Decision Rule

When a review/audit report contains the following patterns, close as "fixed + design intent":

1. "CORS `*.domain` shorthand does not support http; operator config is ineffective"
2. "Recommend reverting to `https?://` for backward compatibility"
3. "Should add an `allowHttpSubdomainWildcard: bool` config"

Verify three points before closing:

- The regex at `cors.rs:167` and `cors.rs:180` is still `^https://[^/]+\.{}$` (not `https?://`)
- The `test_subdomain_wildcard` test still asserts `http://test.example.com` is **not** allowed
- `test_subdomain_wildcard_http_origins_via_regex` still exists, demonstrating that explicit regex is the documented escape hatch

All three satisfied → close per design intent; point to the M13 FIXED decision in `tasks/security-audit/02-access-control.md`.

## Exceptional Scenarios (need reassessment)

- If browser CORS specs revert to encouraging http (extremely unlikely) → reassess
- If Edgion proactively adds an `allowHttpSubdomainWildcard` config field → this rule needs sync update
- If large-scale operator feedback "migration cost is unacceptable" appears → consider adding a new opt-in config field

## Background Notes

- Decision date: 2026-04-21
- Decision source: `tasks/security-audit/02-access-control.md` M13 entry (Option A — code fix, breaking)
- The **first code-layer breaking fix** in this MEDIUM audit batch — contrasts with M4/M8 doc-only:
  - **doc-only applies**: industry mainstream defaults are consistent; changing the default would diverge from the industry
  - **code fix applies**: unintentional behavior of Edgion's bespoke syntactic sugar; not industry convention
- Other review decisions in the same batch:
  - `forward-auth/default-header-forwarding.md` (M7, NOT-AN-ISSUE, aligned with Traefik/Envoy)
  - `oidc/nonce-pkce-defaults.md` (M8, doc-only, aligned with mainstream gateways)
  - This file (M13, code fix, Edgion-unique syntactic sugar)
