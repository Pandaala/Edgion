# GlobalAccessControl Integration Scenario

CDN/origin lockdown plugin. The test client IP is always `127.0.0.1` (loopback).
CIDR allow/deny is exercised by toggling the configured group CIDR in vs out of `127.0.0.0/8`.

## Resources

| File | Resource |
|------|----------|
| `01_Secret_edgion-test_gac-origin-secret.yaml` | `Secret/gac-origin-secret` — CDN origin credential |
| `02_EdgionConfigData_edgion-test_gac-selector.yaml` | `EdgionConfigData/gac-selector` — Selector with `active: lockdown` |
| `03_EdgionPlugins_edgion-test_gac-lockdown.yaml` | `EdgionPlugins/gac-lockdown` — lockdown profile, loopback CIDR (127.0.0.0/8), secret header |
| `04_EdgionPlugins_edgion-test_gac-ip-outside.yaml` | `EdgionPlugins/gac-ip-outside` — lockdown profile, non-loopback CIDR (10.0.0.0/8), secret header |
| `HTTPRoute_edgion-test_gac.yaml` | `HTTPRoute/gac-test-route` — two rules (lockdown path, ip-outside path) |

## Test cases and expected behavior

| Case | Path | Header | Expected |
|------|------|--------|----------|
| 1: `allow_in_cidr_and_secret` | `/test/gac/lockdown/api` | `X-CF-Secret: cf-origin-token-123` | 200 (IP in 127.0.0.0/8, secret matches) |
| 2: `deny_secret_missing` | `/test/gac/lockdown/api` | (none) | 403 (secret absent => NoMatch) |
| 3: `deny_secret_wrong` | `/test/gac/lockdown/api` | `X-CF-Secret: nope` | 403 (wrong token) |
| 4: `deny_ip_outside_cidr` | `/test/gac/ip-outside/api` | `X-CF-Secret: cf-origin-token-123` | 403 (127.0.0.1 not in 10.0.0.0/8) |
| 5: `access_log_allow_decision` | `/test/gac/lockdown/api` | correct header + trace | access log contains `allow:group=cloudflare` |
| 6: `access_log_deny_decision` | `/test/gac/lockdown/api` | no header + trace | access log contains `deny:no-match` |
