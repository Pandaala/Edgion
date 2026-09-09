# External Sync — 879df86f through 312d10e8

- Expected concurrent user commits `879df86f`, `2525b2f0`, and `312d10e8` arrived during B0229. No Gateway source changed or coverage was newly claimed. The independent diagnostic `remote_7825_reconcile`, implementer and final reviewer confirmed B0229 remains safe within its original two-path boundary.
- Root AGENTS now explicitly limits built-in ACME to HTTP-01. DNS provider/TXT probe/credential retirement closes the deleted AliDNS cleanup task by removal, not by repairing a surviving DNS implementation; see `skills/04-review/acme/superseded/dns01-retired.md`. Do not recreate it.
- Preserve external D0101 closure and its standalone task deletion, supported by the new GeoRestriction scenario and `skills/04-review/plugin-runtime/regression-guards/invalid-stream-geoip-generation-fail-closed.md`. Recorded external test results are historical, not this batch's fresh validation. B0175's request-wide PluginStore snapshot proposal was rejected and recorded as maintained design; no source coverage reopening is required solely for that knowledge update.
- The diagnostic verified every following worktree blob against its named source commit and current HEAD, plus file mode. Only these manifest entries were changed; no campaign integration-test edits occurred. Frozen count: 3,122 to 3,120.

| Manifest action | Exact path | Source | Mode |
|---|---|---|---|
| Update hash | `edgion-tests/integration/conf/Services/acme/pebble/docker-compose.yaml` | `879df86f` | regular 0644 / exec 000 |
| Update hash | `edgion-tests/integration/scripts/integration/run_acme_test.sh` | `312d10e8` | regular 0755 / exec 111 |
| Update hash | `edgion-tests/src/client/suites/services/acme/acme.rs` | `879df86f` | regular 0644 / exec 000 |
| Update hash | `edgion-tests/src/client/suites/services/acme/mod.rs` | `879df86f` | regular 0644 / exec 000 |
| Update hash | `edgion-tests/src/client/suites/tcp_route/geo_restriction/geo_restriction.rs` | `2525b2f0` | regular 0644 / exec 000 |
| Remove entry | `edgion-tests/integration/conf/Services/acme/dns01-alidns-example.yaml` | `879df86f` | absent in commit, HEAD and worktree |
| Remove entry | `edgion-tests/integration/conf/Services/acme/dns01-cloudflare-example.yaml` | `879df86f` | absent in commit, HEAD and worktree |

- Freeze check passed after exact reconciliation. Fresh B0229 verification covers the merged workspace; it does not claim execution of the external ACME/GeoRestriction integrations. User-owned handoff/WAF paths remain untouched; never push.
