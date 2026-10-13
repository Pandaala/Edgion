---
name: replay-protection-timestamp-only
description: Use when reviewing hmac_auth replay-protection findings; documents why timestamp-window-only (no nonce dedup) is the correct layering and why gateway-layer nonce storage is not accepted.
knowledge_type: intentional-design
---

# HMAC Auth Replay Protection: Timestamp Window + HTTPS + Business-Layer Idempotency is the Correct Layering

## Conclusion

The `hmac_auth` plugin only does timestamp window check (`clockSkew`, default 300s) and **does not** perform nonce dedup. This is an **already-evaluated design layering**, consistent with mainstream HMAC auth implementations such as AWS SigV4 / Stripe Webhooks / GitHub Webhooks / Kong `hmac-auth` / APISIX `hmac-auth`. It **should not** require introducing nonce storage at the gateway layer (in-memory LRU or Redis).

When a review reports "HMAC Auth should add nonce dedup / should introduce Redis replay-protection storage", close per the rules below.

## Reasons

1. **Industry consistency**:
   - AWS SigV4: timestamp + signature, ~15-minute window, no nonce
   - Stripe Webhooks: timestamp header, 5-minute window, business-layer idempotency
   - GitHub Webhooks: timestamp + signature, no nonce
   - Kong / APISIX `hmac-auth`: timestamp window is the sole defense
   - The "nonce + server-side storage required" pattern of OAuth 1.0a is now a historical approach
2. **Architectural cost of gateway-layer nonce dedup**:
   - **In-memory LRU**: storage is independent across multiple gateway instances; an attacker can replay by switching instances — **illusory defense**
   - **Redis backend**: 1 Redis round-trip per request (~1-3ms latency) + introduces a hard dependency + when Redis fails, fail-open vs fail-close dilemma
3. **Threat model already has layering**:
   - Man-in-the-middle captures signature → blocked by HTTPS (HMAC Auth's basic assumption)
   - Log leaks signature → `hideCredentials: true` + not written to access log
   - Legitimate client malicious replay → **business-layer idempotency key** (correct defense for non-idempotent operations like payment, password change, transfer)
   - Window too large → operator shrinks `clockSkew`
4. **Plugin already has supporting mechanisms**:
   - `clockSkew` is adjustable (recommend shrinking to 60s for sensitive interfaces)
   - `hideCredentials` can be enabled (lowers log-leak surface)
   - `enforceHeaders` can mandate `date` / `host` etc. critical headers entering the signature
5. **Documentation already explains clearly**: `docs/{zh-CN,en}/user-guide/http-route/filters/edgion-plugins/hmac-auth.md` adds a "Replay-Protection Security Model" / "Replay-Protection Security Model" section, writing out the layering, recommended config, and technical reasons for not doing gateway-layer dedup

## Decision Rule

When a review/audit report contains the following patterns, close as "design layering":

1. "HMAC Auth has only a timestamp window; can be replayed indefinitely within 5 minutes; should add a nonce mechanism"
2. "Should support `X-Nonce` header + Redis dedup storage"
3. "Should build in in-memory LRU for nonce recording"
4. "Should follow the OAuth 1.0a nonce model"

Verify three points before closing:

- `hmac_auth/plugin.rs`'s `authenticate_request` still only does `validate_clock_skew` + `verify_signature` (no nonce-store or dedup call)
- `HmacAuthConfig` has no `require_nonce` / `nonce_header` field
- The user-doc "Replay-Protection Security Model" / "Replay-Protection Security Model" section is still present, explicitly stating industry layering and the technical reasons for not doing gateway-layer dedup

All three satisfied → design layering; point to the M6 FIXED (doc-only) decision in `tasks/security-audit/01-auth-plugins.md` and close.

## Exceptional Scenarios (need reassessment)

- Industry mainstream (AWS / Stripe / GitHub) shifts to enforcing nonce at the gateway / SDK layer → reassess and follow
- Concrete cases of "business layer cannot do idempotency, must rely on gateway dedup" feedback from real deployments appear → assess opening a separate project to add an optional Redis backend
- Security compliance (e.g., PSD2 strong customer authentication) mandates gateway-layer nonce → start a separate project per compliance

## Cross-Reference with Other Opt-In / Doc-Only Decisions

This decision belongs to the same category as the following review rules: "**layered decoupling + documentation guides operators to make corresponding configurations**":

- `auth-timing/basic-auth-username-enumeration.md` (M4) — doc recommends `authFailureDelayMs` + RateLimitLocal combination
- `oidc/nonce-pkce-defaults.md` (M8) — doc recommends enabling `useNonce` / `usePkce` in production
- `real-ip/max-trusted-hops-opt-in.md` (M12) — doc recommends two-layer defense (precise CIDRs in IpLists named by trustedIpRefs + maxTrustedHops cap)
- This file (M6) — doc recommends HTTPS + hideCredentials + business-layer idempotency

Common pattern: **code maintains industry consistency + documentation makes threat layering explicit + operator does corresponding deployment per docs**.

## Background Notes

- Decision date: 2026-04-22
- Decision source: `tasks/security-audit/01-auth-plugins.md` M6 (Option C — doc-only)
- Key technical fact: the multi-instance consistency problem of gateway-layer nonce dedup is an unavoidable architectural dilemma; simply "adding Redis" pushes this dilemma onto the operator
