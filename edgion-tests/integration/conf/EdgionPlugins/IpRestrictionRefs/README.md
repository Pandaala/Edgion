# EdgionPlugins / IpRestrictionRefs

End-to-end coverage for the `IpRestriction` plugin's `allowRefs` / `denyRefs` fields,
which let the plugin delegate its IP group data to a central `IpList` `EdgionConfigData`
instead of embedding inline `allow` / `deny` entries.

All cases set `ipSource: RemoteIp`, so the checked IP is the resolved address, which is
the deterministic TCP peer when no forwarded address is supplied.

| Path | Config | Expected |
|------|--------|----------|
| `/test/allow-ref-hit` | `allowRefs: [{name: ipref-allowlist}]` — list contains `127.0.0.0/8` | **200** — client IP in referenced allow list |
| `/test/deny-ref-hit`  | `denyRefs: [{name: ipref-blocklist}]`, `defaultAction: allow` — list contains `127.0.0.0/8` | **403** — client IP in referenced block list |

The two `EdgionConfigData` objects (prefix `01_`) are applied before the `EdgionPlugins`
objects (prefix `02_`) so the refs resolve at admission time.

Run: `./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r EdgionPlugins -i IpRestrictionRefs`
