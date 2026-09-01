# B0099 — RegionRoute

## Scope
- Baseline: `e391354b`
- Pipeline state: commit-ready
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/region_route/{log_consts.rs,mod.rs,plugin.rs}`
- Implementation allow-list: `edgion-gateway/src/plugins/http/region_route/plugin.rs`, `edgion-resources/src/resources/edgion_plugins/plugin_configs/region_route.rs`, and the English/Chinese RegionRoute user-guide pages
- Forbidden files: `log_consts.rs`, `mod.rs`, Controller, PluginRuntime/session/context, region stores and APIs, other documentation, Cargo, every existing `edgion-tests/**` path, and protected user paths

## Analysis boundary
- Trace resources validation, Controller/Gateway preparse parity, PluginRuntime sealing, generation overlays, hash-key extraction and calculation, region selection/failover, jump publication, outbound peer security, logs, tests, integration, and documentation.
- Preserve stable integration log tokens, fail-to-base and overlay generation behavior, sensitive key handling, generic sealed 500 responses, DNS and post-resolution address validation, hot reload, no-tracing rules, and the extracted Center product boundary.
- Existing integration tests remain frozen. Record a separate issue only if a genuine future integration gap cannot be covered deterministically in colocated tests.

## Handoff
- Analyst: independent read-only analysis completed across external HEAD drift; all three anchor blobs remained unchanged when promoted on `e391354b`
- Decision: simple correctness fix; reject `hashCalc.modulo < 2` canonically and retain a defensive nonzero request-path boundary so direct construction cannot panic
- Supporting scope: resources validation plus the dedicated English/Chinese RegionRoute pages are the exact minimum required for schema/preparse/documentation parity
- Deferred: D0065 owns full base/overlay/effective-table validation and must not be partially implemented here
- Implementation: exact four-file allow-list honored; resources, Gateway, parity, and Controller RegionRoute tests plus targeted checks and guards passed
- Final review: independently approved with no findings after proving the defensive branch is terminal before key extraction or side effects and canonical validation seals invalid resources with the existing generic 500
- Coverage: `plugin.rs` changed; `log_consts.rs` and `mod.rs` examined-no-change
- Verification: 10 resources RegionRoute tests, 23 Gateway RegionRoute tests, 1 PluginRuntime validation-parity test, and 18 Controller RegionRoute tests passed; targeted crate checks, format, locked workspace check/Clippy, Gateway build, agent-doc, SSA, metrics, Pingora source, no-tracing, English-only, diff, and frozen-path guards passed
- Integration: the frozen `edgion-tests/**` tree remained unchanged; colocated validation and sealed-runtime coverage is sufficient, so no integration issue was created
- Commit: pending
