# EdgionPlugins / IpRestrictionPartialGood

End-to-end, traffic-based proof that **partial-good `IpList` compilation** (Tasks 1-8)
holds on the data plane: an `IpList` whose CIDR group mixes one VALID and one INVALID
entry still enforces the valid entry — the invalid one is skipped, not fatal. There is no
deny-all collapse and no dropped plugin.

All cases set `ipSource: remoteAddr`, so the checked IP is the deterministic TCP peer of
the test client (`127.0.0.1`), independent of RealIp / X-Forwarded-For processing. Both
referenced `IpList` `EdgionConfigData` objects contain a deliberately invalid CIDR sibling.

This scenario covers two partial-good IpList paths:

- **Referenced (`denyRefs`)** — the IP groups live in `EdgionConfigData` (`IpList`) objects
  referenced by the plugin.
- **Inline (`deny`)** — the IP group is carried directly in the plugin's inline `deny`, with
  no `EdgionConfigData` ref. This proves the inline CIDR-compile path is also partial-good.

| Path | Source group (good + bad CIDRs) | Expected |
|------|---------------------------------|----------|
| `/test/partial-deny-hit`  | `denyRefs → ippg-blocklist-hit` — `["127.0.0.0/8", "not-a-cidr"]`, `defaultAction: allow` | **403** — the good CIDR compiled despite the bad sibling, client is a deny hit |
| `/test/partial-deny-miss` | `denyRefs → ippg-blocklist-miss` — `["10.0.0.0/8", "999.0.0.1/8"]`, `defaultAction: allow` | **200** — list compiled to `{10.0.0.0/8}` (NOT deny-all); client misses → default allow |
| `/test/inline-deny-hit`   | inline `deny` — `["127.0.0.0/8", "not-a-cidr"]`, `defaultAction: allow` | **403** — the good inline CIDR compiled despite the bad inline sibling, client is a deny hit |
| `/test/inline-deny-miss`  | inline `deny` — `["10.0.0.0/8", "not-a-cidr"]`, `defaultAction: allow` | **200** — inline deny compiled to `{10.0.0.0/8}` (NOT deny-all); plugin loaded, did not init_failed; client misses → default allow |

The `403` cases prove acceptance #1/#2 (a valid entry compiles next to a bad one). The `200`
cases prove the list/plugin did not collapse to deny-all on the bad entry — for the inline
miss case, a plugin that had failed init would deny ALL traffic (403), so the `200` is direct
proof the plugin LOADED.

K8s status-condition (`PartiallyInvalid=True`) assertions are intentionally NOT made here —
those are covered by unit tests in Tasks 4 and 7. This scenario is traffic-only, mirroring
`IpRestrictionRefs`.

The two `EdgionConfigData` objects (prefix `01_`) are applied before the `EdgionPlugins`
objects (prefix `02_`) so the refs resolve at admission time.

Run: `./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r EdgionPlugins -i IpRestrictionPartialGood`
