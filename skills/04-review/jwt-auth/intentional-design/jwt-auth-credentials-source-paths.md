---
name: jwt-auth-credentials-source-paths
description: Decide on jwt_auth `load_credentials_from_secret` "4-path duplication / should extract helper" findings. Audited design tradeoff, will not fix.
knowledge_type: intentional-design
---

# jwt_auth credentials.rs 4-Path Credential Loading — Audited Design Tradeoff

**Audit date**: 2026-04-26
**Linked issue**: P2-12 (closed, not fixed)
**Conclusion**: **The 4 branches of `edgion-gateway/src/plugins/http/jwt_auth/credentials.rs::JwtAuth::load_credentials_from_secret` keep their current shape; "extract helper / eliminate duplication / shorten the main function" fix suggestions are not accepted.**

## Code Involved

- `edgion-gateway/src/plugins/http/jwt_auth/credentials.rs:66-186`
  - Branch 1: `config.resolved_credential` (controller-pre-resolved single credential)
  - Branch 2: `config.resolved_credentials` (controller-pre-resolved multi-credential map)
  - Branch 3: `config.secret_ref` (fallback: get single credential from K8s Secret)
  - Branch 4: `config.secret_refs` (fallback: get multiple credentials from K8s Secret)

## Points That Look Like Issues (will not be fixed)

All 4 paths perform symmetric/asymmetric branching + base64 decoding + UTF-8/PEM extraction + error message construction; the function is ~120 lines. On the surface, a textbook "should extract a helper".

## Why It Will Not Be Changed

1. **"Duplication" is structural rather than real**: the 4 paths differ in data source type
   - resolved path: `String` (already base64-encoded; the asymmetric branch only does `pem.clone()`)
   - secret path: `Vec<u8>` (raw K8s Secret bytes; the asymmetric branch needs UTF-8 conversion)
   - Error message label formats differ ("Resolved credential" vs "Secret {ns}/{name}" vs "Resolved credential '{key}'")
   - Extracting a helper inevitably parameterizes "how to fetch data + error label format"; the helper itself becomes complex; saved code = a few dozen lines, indirection added = one layer

2. **Not on the hot path**: `load_credentials_from_secret` is cached by `load_credentials` (`self.credentials: RwLock<Option<Credentials>>`); called only once per JwtAuth instance. Performance and readability demands are weak.

3. **Regression risk is real**: error wording is verified character-by-character by 25 integration tests (`tests/secret_decoding.rs`, `tests/algorithm.rs`, etc.). The refactoring benefit is a few dozen lines of code; the cost is carefully aligning error wording + running the full test suite.

4. **Structure is stable**: the 4 paths are determined by schema (resolved single/multi, fallback single/multi); there will not be a 5th path. The "future extension" motivation does not hold.

## Applicable Decision Rule

In future reviews, the following findings should be closed directly without re-evaluation:

- "`load_credentials_from_secret` 4-path duplication; should extract `build_credential_from_*` helper"
- "120-line function is too long; should be split"
- "symmetric/asymmetric branching is written 4 times; violates DRY"

If a **new dimension** is found (e.g., the 4 paths behave inconsistently under some boundary condition; the controller-resolved and fallback get_secret paths have an undocumented semantic difference), that is a different finding and should be evaluated on its own merits.
