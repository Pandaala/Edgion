# Webhook Key Resolver Has No Response Signature Verification: Relying on TLS is the Industry Pattern

## Conclusion

`webhook/resolver.rs` not performing HMAC / signature verification on the HTTP response from the webhook service is a **deliberate architectural tradeoff** — consistent with the industry "actively call external services" category (OAuth Introspection, OPA ExtAuthz), relying on TLS + a legitimate CA as the sole defense line. It **should not** require introducing a `responseSignature` configuration field or enforcing signature verification.

## Reasons

1. **Semantic direction is inverse to traditional webhooks**:
   - GitHub / Stripe / Slack "webhook signatures" are **sender→receiver direction** (sender signs, receiver verifies); they exist because the receiver cannot actively verify the sender's identity
   - Edgion `WebhookKeyGet` is the **reverse** — Edgion actively initiates an HTTP call to an operator-statically-configured URI, similar to OAuth Introspection (RFC 7662) or OPA ExtAuthz
   - The active initiator can verify server identity via the TLS handshake; no extra signing layer needed
2. **Industry consistency**:
   - OAuth Introspection (RFC 7662): no response signature spec; consistently relies on TLS
   - OPA External Authorization: no response signature
   - Kong webhook-like plugins: no response signature
   - All rely on TLS + PKI
3. **Signature is fully redundant when TLS is configured correctly**:
   - MITM / DNS hijacking are blocked by the TLS handshake under `https://` + a legitimate CA
   - An attacker cannot forge a certificate for the target domain
4. **The only value of a response signature = backstop for misconfigured TLS**:
   - Only relevant if the operator set `insecure_skip_verify: true` or used an `http://` URI
   - That is operator error, not a code gap; the correct fix is to fix the configuration, not patch incorrect configuration with a backstop
5. **Webhook TLS support is complete**: a unified `TlsConfig { enabled, certs, insecure_skip_verify }` + mTLS via `secretRef` provides full capability; the operator should use it
6. **URI is statically configured by the operator**: not user-controllable input; there is no attack surface of "user induces Edgion to call a malicious endpoint"; consistent with the H3 ForwardAuth/Webhook SSRF NOT-AN-ISSUE judgment

## Decision Rule

When a review/audit report contains the following patterns, close as "industry pattern consistency + real defense in TLS":

1. "Webhook response is unsigned; MITM can forge key resolution result"
2. "Should introduce `responseSignature` / `X-Edgion-Signature` HMAC verification field"
3. "Should mandate that all webhook responses must be signed"
4. "Should build a common response signature verification pattern into the resolver"

Verify three points before closing:

- `webhook/resolver.rs::extract_value_from_response` still returns the extracted value directly, with no signature verification call
- `WebhookProviderConfig` has no `responseSignature` / `signatureHeader` field
- The `TlsConfig { enabled, certs, insecure_skip_verify }` quartet (including `certs.secret_ref`) is still complete

All three satisfied → architectural tradeoff; point to `tasks/security-audit/04-admin-api.md` L1 NOT-AN-ISSUE decision and close.

## Exceptional Scenarios (need reassessment)

- Upstream specs like OAuth Introspection / OPA ExtAuthz introduce a response signature spec → assess and follow
- A compliance requirement (certain industries) mandates that webhook responses must be signed → start a separate project per compliance
- Edgion changes the webhook URI to allow user input (e.g., a `${header.x-target}` template) → it is no longer operator-statically-configured; the threat model changes; reassess

## Cross-Reference with Other NOT-AN-ISSUE / Architectural Tradeoff Decisions

- `ssrf/forward-auth-webhook-ssrf.md` (H3) — same "URI statically configured by operator" judgment basis
- `forward-auth/default-header-forwarding.md` (M7) — same "design tradeoff aligned with Traefik / Envoy mainstream"
- `hmac-auth/replay-protection-timestamp-only.md` (M6) — same "no new gateway-layer infrastructure; rely on TLS + industry layering"

Common pattern: **Edgion does not stack redundant, non-industry-standard security layers in the proxy layer; it trusts existing layering (TLS / PKI / RBAC / NetworkPolicy) and lets the operator configure correctly**.

## Background Notes

- Decision date: 2026-04-22
- Decision source: `tasks/security-audit/04-admin-api.md` L1 (Option D — NOT-AN-ISSUE)
- Key fact: the webhook key resolver is "Edgion actively calls an external service" rather than "receiver passively receives a sender push"; signature semantics are completely different
