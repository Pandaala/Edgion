# EdgionPlugins / IpRestriction

End-to-end coverage for the HTTP-layer `IpRestriction` plugin, added as the regression
for **fr-types-01** (the plugin was built via `IpRestriction::create` without
`validate_and_init`, so the runtime matchers were never compiled and the evaluator fell
through to `Default(Allow)` — a whitelist silently allowed every IP).

All cases set `ipSource: remoteAddr`, so the checked IP is the deterministic TCP peer of
the test client (`127.0.0.1`), independent of RealIp / X-Forwarded-For processing.

| Path | Config | Expected |
|------|--------|----------|
| `/test/allow-miss` | allow `10.0.0.0/8` (client not in list) | **403** — the fr-types-01 fail-open regression |
| `/test/allow-hit`  | allow `127.0.0.0/8` (client in list) | 200 |
| `/test/deny-hit`   | deny `127.0.0.1/32`, default allow | 403 |
| `/test/deny-miss`  | deny `10.0.0.0/8`, default allow | 200 |

Run: `./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r EdgionPlugins -i IpRestriction`
(requires the gateway binary to be rebuilt with the fr-types-01 fix).
