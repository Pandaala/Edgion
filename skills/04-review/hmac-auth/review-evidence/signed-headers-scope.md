---
name: signed-headers-scope
description: Decision rule for HMAC Auth "signature only covers declared headers / unsigned fields can be tampered with" findings. Read this file first.
knowledge_type: review-evidence
---

# HMAC Auth Signature Only Covers Declared Headers — NOT-AN-ISSUE

## Conclusion

"HMAC Auth does not enforce signing all headers; fields not declared in `enforce_headers` can be tampered with without breaking the signature" — classified as NOT-AN-ISSUE. Fix suggestions of the form "enforce signing all headers by default" or "add Content-Type to the default signed list" are not accepted.

## Core Rationale

**1. This is the design semantic of the HMAC Auth protocol**

The core model of HMAC Auth is: the signer explicitly declares which fields enter the signature scope; the verifier verifies with the same declaration. This is consistent with industry standards:

- **AWS Signature Version 4 (SigV4)**: the `SignedHeaders` field is explicitly listed; headers not listed are not within signature protection
- **Stripe webhook**: only signs the payload body; HTTP headers do not enter the signature
- **GitHub webhook**: only signs the body; headers do not enter the signature
- **Kong `hmac-auth` plugin**: same pattern as Edgion; the operator declares which headers enter the signature

**2. Clear responsibility division**

Headers not declared being outside HMAC protection is **a known, explicit semantic**, not a code oversight. If the operator needs to protect a certain header (e.g., `Content-Type`, `X-Custom-Action`), they should add it to the `enforce_headers` config. Documentation explains this mechanism.

**3. Cost of default full signing**

If all headers were forced to be signed, any legitimate header modification by transparent proxies (e.g., adding `Via`, adjusting `Transfer-Encoding`) would cause verification to fail, severely affecting availability.

## Fix Suggestions Not Accepted

- Add `Content-Type` or other headers to the default signed list (breaks compatibility with existing HMAC clients)
- Force signing all headers by default (transparent proxy header modification will break verification)
- Warn that "unsigned fields can be modified" (this is the design semantic, not a defect)

## Correct Configuration Guidance

The operator should add all business-critical headers to `enforce_headers`:
```yaml
enforceHeaders:
  - date
  - content-type
  - x-custom-action
```
