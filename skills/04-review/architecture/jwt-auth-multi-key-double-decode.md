---
name: jwt-auth-multi-key-double-decode
description: Decide on jwt_auth multi-key path `verify_token` "decodes payload twice / should look up key from JWT header (kid) to avoid the unverified peek decode" findings. Audited design tradeoff, will not fix.
---

# jwt_auth Multi-Key Path Double Payload Decode — Audited Design Tradeoff

**Audit date**: 2026-05-15
**Linked issue**: P3-g3-017 (closed, not fixed)
**Conclusion**: **The multi-key branch in `src/core/gateway/plugins/http/jwt_auth/verify.rs::JwtAuth::verify_token` keeps its current "decode-twice (no-verify peek, then verified decode)" shape; fix suggestions that move credential selection to JWT header `kid` are not accepted.**

## Code Involved

- `src/core/gateway/plugins/http/jwt_auth/verify.rs:83-129` — `Credentials::Multi` branch
  - First `decode::<Claims>` with `insecure_disable_signature_validation` → reads the `key_claim_name` value from the payload to look up the matching credential in the multi-key map.
  - Second `decode::<Claims>` with full validation → produces the verified claims that downstream code consumes.
- `src/types/resources/edgion_plugins/plugin_configs/jwt_auth.rs:93-95` — `key_claim_name` field documented as "Claim name in JWT **payload** that identifies the credential key (default: `key`)".
- `docs/en/dev-guide/jwt-auth-plugin-design.md` / `docs/zh-CN/dev-guide/jwt-auth-plugin-design.md` — `key_claim_name` semantics explicitly aligned with APISIX/Kong.

## Points That Look Like Issues (will not be fixed)

The same JWT payload is base64-decoded and serde-deserialized into `Claims` twice per request on the multi-key branch. Surface reading: a textbook "redundant work, move the selector to the already-decoded header (`kid`) to eliminate the peek decode".

## Why It Will Not Be Changed

1. **Inspection-before-verify is structural in multi-key, not a coding mistake**:
   - The verification key must be chosen before signature verification runs.
   - The selector lives in the user-configured `key_claim_name` claim, which is a payload field by design.
   - The alternatives are all worse: trying every key in the map (O(N) verifications + key-enumeration / timing surface), or moving the selector to a header field (see point 2).

2. **The "move to `kid`" suggestion breaks documented semantics and ecosystem alignment**:
   - `key_claim_name` is a user-facing config string documented as a payload claim name in both en/zh design docs and the schema field comment.
   - APISIX and Kong both use the payload-claim model. Switching to header `kid` lookup misaligns with those ecosystems and breaks existing tokens issued by deployments that already encode the selector in a payload claim (often `iss`, `sub`, `consumer`, `tenant`, etc.).
   - A non-breaking variant requires a new config field (e.g. `keyClaimInHeader: bool`) plus tests plus doc updates — medium-cost change for a microsecond-scale gain.

3. **Security is not improved by header lookup**:
   - In both designs the key selector is read from unverified bytes (payload or header) before signature verification.
   - The signature check still catches forgery in the verified second decode either way; there is no security delta between the two locations.

4. **Performance impact is negligible and bounded to a cold sub-branch**:
   - The double decode runs only when `secret_refs` is configured with multiple credentials — the default `secret_ref` single-key path is unaffected.
   - JWT payload is typically <1KB; the second JSON parse + `Claims` deserialization is microseconds.
   - jwt-auth is a per-request L7 plugin, not a tight inner loop.

5. **Lossy "lighter peek" optimization (manual base64 + `serde_json::Value::get`) is also rejected**:
   - The current code reuses the same `Claims` deserialization on both decodes, which keeps the malformed-token classification path identical (`classify_jwt_decode_error` / `"Deny malformed token"`).
   - A divergent manual peek would create two malformed-token paths and risk drift from the 25 integration tests that lock error wording character-by-character.

6. **Key-selection and username-extraction reuse the same configured claim**:
   - `verify.rs:147-153` already uses `key_claim_name` (with `sub` fallback) for username extraction. One config knob solves both "pick the credential" and "produce the username".

## Design Benefits That Justify the Cost

- Reuses existing issuer-side claims (`iss`, `sub`, `consumer`, `tenant`, `preferred_username`) without forcing the issuer to inject a header field for the gateway.
- Decouples "consumer identifier" (business semantics) from `kid` (JWK key rotation metadata in the spec).
- Configurable: `key_claim_name` can point to any payload field; `kid` would be a fixed header field with no operator flexibility.
- Consistent malformed-token classification across the no-verify peek and the verified decode (single `Claims` codec path).
- Ecosystem parity with APISIX/Kong → low migration cost for operators.

## Applicable Decision Rule

In future reviews, the following findings should be closed directly without re-evaluation:

- "Multi-key path decodes the JWT payload twice; should use `kid` from the JWT header to avoid the second decode"
- "`insecure_disable_signature_validation` peek decode is redundant work / hot-path waste"
- "Should replace the no-verify decode with a manual base64 + `serde_json::Value` lookup to skip the `Claims` deserialization"

If a **new dimension** is found (e.g., the no-verify peek branch and the verified decode disagree on malformed-token classification, or a measurable performance regression is shown on a realistic multi-key workload with profiler data), evaluate it on its own merits as a different finding.
