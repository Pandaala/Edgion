# RequestAccessPolicy Integration Scenario

This scenario verifies an IP-first `directPeerIp` rule combined with a Secret-backed header condition.

| File | Purpose |
|---|---|
| `01_Secret_edgion-test_gac-origin-secret.yaml` | Origin token Secret |
| `02_EdgionConfigData_edgion-test_gac-selector.yaml` | Active-profile Selector |
| `03_EdgionPlugins_edgion-test_gac-lockdown.yaml` | Loopback IP plus correct token allows |
| `04_EdgionPlugins_edgion-test_gac-ip-outside.yaml` | Non-matching IP denies |
| `HTTPRoute_edgion-test_gac.yaml` | Route attachments |

The suite covers successful IP-plus-secret authorization, missing or wrong Secret values, an IP-list miss, and allow/deny access-log decisions.
