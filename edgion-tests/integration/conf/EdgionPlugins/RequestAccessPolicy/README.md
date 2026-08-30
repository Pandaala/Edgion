# RequestAccessPolicy Integration Scenario

This scenario verifies an IP-first `directPeerIp` rule combined with a Secret-backed header condition, plus the canonical nested HMAC match contract.

| File | Purpose |
|---|---|
| `01_Secret_edgion-test_gac-origin-secret.yaml` | Origin token Secret |
| `02_EdgionConfigData_edgion-test_gac-selector.yaml` | Active-profile Selector |
| `03_EdgionPlugins_edgion-test_gac-lockdown.yaml` | Loopback IP plus correct token allows |
| `04_EdgionPlugins_edgion-test_gac-ip-outside.yaml` | Non-matching IP denies |
| `05_Secret_edgion-test_gac-hmac-credentials.yaml` | HMAC credential with metadata that policy matching must ignore |
| `06_EdgionPlugins_edgion-test_gac-hmac.yaml` | Signature, body-Digest, and sibling-fallback policy |
| `07_HTTPRoute_edgion-test_gac-hmac.yaml` | HMAC policy route attachment |
| `HTTPRoute_edgion-test_gac.yaml` | Route attachments |

The suite covers successful IP-plus-secret authorization, missing or wrong Secret values, an IP-list miss, allow/deny access-log decisions, HMAC allow/deny behavior, boolean fallback, body forwarding after a Digest mismatch, and the absence of standalone header mutations.
