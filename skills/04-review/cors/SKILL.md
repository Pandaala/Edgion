---
name: review-cors
description: Review knowledge for cors, grouped by decision type.
---

# cors review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [Native credentialed CORS requires explicit exposure names](intentional-design/native-credentialed-exposure-policy.md)

- [Native CORS belongs to the route rule](intentional-design/native-cors-rule-scope.md)
- [CORS response-header values borrow; no runtime cache fields](intentional-design/response-header-values-borrowed.md)
- [CORS `*.domain` Shorthand Matches HTTPS Only (Fixed)](intentional-design/subdomain-wildcard-https-only.md)

## Regression guards

- [CORS method and header token-list validation](regression-guards/token-list-validation.md)
- [Native Gateway API CORS validation matches v1.5 on every ingest path](regression-guards/native-v15-validation-parity.md)
- [Preflight policy follows EGC hot reload at the request boundary](regression-guards/preflight-policy-egc-hot-reload.md)
- [CORS Actual Responses Carry No Preflight-Only Headers — FIXED](regression-guards/actual-response-preflight-only-headers.md)
- [AllOptions Preserves Route CORS Ownership — FIXED](regression-guards/all-options-route-cors-pass-through.md)
- [CORS Force-Wildcard (`**`) Does Not Emit `tracing::warn!` (Fixed)](regression-guards/force-wildcard-no-tracing-warn.md)
- [CORS `get_validated_origin` Split — Value Selection Extracted (Fixed)](regression-guards/get-validated-origin-split.md)
- [Global CORS Preflight Result Mapping — FIXED](regression-guards/gw-http-proxy-req-01-preflight-result-mapping.md)
- [CORS ExtensionRef Scan Skips Malformed Entries — FIXED](regression-guards/gw-http-proxy-req-02-extension-ref-scan-skips-malformed-entry.md)
- [CORS `Cors::new` Compiles Origin Matchers — FIXED](regression-guards/new-must-call-validate-and-init.md)
- [Route CORS Preflight Has One Runtime Owner — FIXED](regression-guards/preflight-route-cors-single-owner.md)
- [CORS Regex Allowlists Rejected With `allow_credentials` (Fixed)](regression-guards/regex-origins-with-credentials-rejected.md)
