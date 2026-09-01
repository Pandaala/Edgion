# Align JWE Resolved-Credential Failure Policy

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | design-correctness |
| Priority | P1 |
| Issue | N/A |
| Depends on | B0084 |

## AI guide summary

Preserve operator-visible validation status while making malformed resolved JWE credential material produce the canonical generic authentication denial rather than the current invalid-plugin HTTP 500.

## Requirements

- Design one cross-crate contract for resource validation, Controller status, plugin publication, and Gateway prepared credential state.
- Treat invalid outer Base64, invalid optional inner Base64, and wrong decoded key length consistently.
- Preserve resolved-credential precedence and never fall back to a stale or global Secret when resolved material is present but invalid.
- Keep all client responses generic and all logs free of Secret bytes, key material, tokens, and plaintext.
- Define Secret rotation recovery and last-known-good behavior before implementation.
- Do not modify existing integration tests during design; create a separate integration issue only if focused cross-crate coverage cannot prove the approved contract.

## Acceptance criteria

- An approved design reconciles authentication failure policy with resource status signaling and immutable generation publication.
- Unit and cross-crate tests cover malformed material, unavailable material, valid replacement, and generation isolation.
- Gateway format/check/Clippy and repository guards pass after implementation.
