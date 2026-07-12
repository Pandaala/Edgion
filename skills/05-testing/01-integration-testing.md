# Edgion Integration Testing

Use for these tasks:

- Running or narrowing down local integration tests
- Adding a new `edgion-tests/` case
- Investigating "YAML loaded but gateway behaves wrong / test result unstable / cert or backend behavior mismatched"

Read this page first to get the main workflow; for specific mappings read these next:

- Suite / directory mapping: [references/integration-suite-map.md](references/integration-suite-map.md)
- `test_server` capability list: [references/test-server-capabilities.md](references/test-server-capabilities.md)
- Troubleshooting after preserving the scene: [../06-tracing/00-debugging.md](../06-tracing/00-debugging.md)

## macOS prerequisite: lo0 alias

`HTTPRoute/Backend/LB*` tests use `127.0.0.2` / `127.0.0.3` / `127.0.0.4` as multiple independent endpoints for LeastConn / RoundRobin / ConsistentHash. Linux routes the entire `127/8` to `lo` by default, so CI (ubuntu-latest) works out of the box; **on macOS, by default `lo0` only binds `127.0.0.1`**, so an alias must be added to `lo0` manually, otherwise connect silently times out (test_server binding `0.0.0.0` cannot save it).

`run_integration.sh` performs a preflight check at startup. If the alias is missing it exits and prints the `sudo ifconfig lo0 alias …` command — run it manually once as instructed. The alias is lost after a reboot and must be re-added; for permanent effect use a launchd plist (instructions in the prompt).

## Preflight quality gates

Integration is the project's key control point, so `run_integration.sh` enforces three repo-wide quality gates at the very top of `main()`, in order. **Any failure aborts with `exit 1` before any build or test runs**, and they run on every invocation — including `--no-prepare` (only the build step is gated by prepare, not these gates).

| Gate | Entry point | Enforces |
|------|-------------|----------|
| English-Only Lint | `cicd/checks/check_english_only.sh` | no non-English letter scripts outside `docs/` and `tasks/` |
| Agent-doc Validation | `python3 cicd/checks/validate_agent_docs.py` | `AGENTS.md` / `skills/` links, frontmatter, indexes, stale `src/` refs |
| Types Purity | `cicd/checks/check_types_purity.sh` | `edgion-resources` stays a clean leaf crate (no `crate::core::` edges) |

These are the same `cicd/checks/*` entry points CI uses — there is no `Makefile` (the former `make check-*` targets were dissolved into standalone `cicd/` scripts). To reproduce a gate failure standalone, run its entry point directly, e.g. `python3 cicd/checks/validate_agent_docs.py`. A common cause is doc-vs-code drift after a refactor (e.g. a `skills/` file referencing a moved/deleted `src/` path) — fix the reference, not the gate.

## Quick start

```bash
# Full local integration test
./edgion-tests/integration/scripts/integration/run_integration.sh

# Run only one resource family
./edgion-tests/integration/scripts/integration/run_integration.sh -r HTTPRoute

# Run only one item
./edgion-tests/integration/scripts/integration/run_integration.sh -r TLSRoute -i MultiSNI

# Already built, skip prepare
./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r EdgionPlugins -i KeyAuth

# Keep the scene after the run for manual investigation
./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare --keep-alive -r Gateway -i StreamPlugins
```

### Run every integration test in one shot

`run_integration.sh` only covers the suites that share its single controller/gateway topology. Several other tests live in standalone scripts (Etcd / Elasticsearch LinkSys, ACME, Center watch-sync, edgion-cli E2E, FileWatcher) — each with its own service lifecycle and port layout. To exercise *everything* end-to-end use the sequential orchestrator:

```bash
# Run every job in fast mode, continue on failure, unified report at the end
./edgion-tests/integration/scripts/integration/run_all_integration.sh

# Skip the top-level cargo build (children may still rebuild incrementally)
./edgion-tests/integration/scripts/integration/run_all_integration.sh --no-prepare

# Propagate --full-test to the main runner (LdapAuth, slow timeouts, etc.)
./edgion-tests/integration/scripts/integration/run_all_integration.sh --full-test

# Limit the run
./edgion-tests/integration/scripts/integration/run_all_integration.sh --only main,linksys_etcd
./edgion-tests/integration/scripts/integration/run_all_integration.sh --skip acme,center
```

Each job's stdout/stderr is captured to `tmp/integration_testing/run_all_<timestamp>/<job>.log` plus a machine-readable `summary.txt`. Job names: `main`, `linksys_etcd`, `linksys_es`, `acme`, `center`, `cli`, `file_watcher`. All children share controller/gateway ports (12101/12001) so they run sequentially even when their topologies differ.

## Memorize the actual directory layout

- `edgion-tests/integration/conf/`: test YAML, organized by resource/item
- `edgion-tests/src/client/`: Rust test client and suite implementations
- `edgion-tests/src/server/test_server.rs`: local HTTP/gRPC/WebSocket/TCP/UDP test backend
- `edgion-tests/src/validator/`: `resource_diff`, `config_load_validator`
- `edgion-tests/integration/scripts/integration/run_integration.sh`: main entry (shared controller/gateway suites)
- `edgion-tests/integration/scripts/integration/run_all_integration.sh`: top-level orchestrator that sequences `run_integration.sh` plus every standalone integration script (Etcd / ES LinkSys, ACME, Center, CLI, FileWatcher) and emits a unified report
- `edgion-tests/integration/scripts/utils/`: `prepare.sh`, `start_all_with_conf.sh`, `load_conf.sh`, `kill_all.sh`
- `edgion-tests/integration/scripts/gen_certs/`: test certificate generation scripts
- `tmp/integration_testing/testing_<timestamp>/`: this run's logs, PIDs, generated Secrets, and report directory

Do not search for code or cert scripts using older test-directory conventions. Current code lives in `edgion-tests/src/`; cert scripts are in `edgion-tests/integration/scripts/gen_certs/`.

## Run flow

The actual responsibilities of `run_integration.sh` are:

1. Call `prepare.sh` to build `edgion-controller`, `edgion-gateway`, `edgion-cli`, `test_server`, `test_client`, `test_client_direct`, `resource_diff`, `config_load_validator`
2. Call `start_all_with_conf.sh` to clean the scene and create `tmp/integration_testing/testing_<timestamp>/`
3. Generate runtime Secrets under `${WORK_DIR}/generated-secrets/` and load them once after the suite YAML
4. Start `test_server`, controller, gateway, and wait for health checks
5. First load `edgion-tests/integration/conf/base/`, then load the configuration of the target suite, and finally load runtime-generated Secrets
6. Use `resource_diff` to verify the controller↔gateway sync state
7. Call `test_client -g -r <Resource> -i <Item>` to run the tests
8. Clean up by default; with `--keep-alive` the processes, logs, and working directory are kept

A few key facts:

- `--full-test` additionally includes slow tests and requires Docker to be available
- `run_integration.sh` first checks `ulimit -n` and tries to raise it to `65535` if insufficient
- `start_all_with_conf.sh` exports `EDGION_WORK_DIR` and `EDGION_GENERATED_SECRET_DIR`
- `load_conf.sh` skips template Secrets; the generated version in the working directory takes precedence at the end

## Categorize first, then write the test

The current `test_client` local integration tests are mainly grouped into these families:

- `HTTPRoute`: Basic, Match, Backend, Filters, Protocol
- `GRPCRoute`: Basic, Match
- `Gateway`: Security, RealIP, AllowedRoutes, TLS, DynamicTest, StreamPlugins, PortConflict, Combined
- `TCPRoute` / `TLSRoute` / `UDPRoute`
- `EdgionPlugins`
- `EdgionTls`
- `ReferenceGrant` status tests
- `Services` (e.g., ACME) and the standalone path of `LinkSys`

The corresponding YAML directory, Rust suite, and whether `--gateway` is required are in [references/integration-suite-map.md](references/integration-suite-map.md).

## Adding or modifying a local integration test

### 1. Pick the closest existing suite

Look for the closest template:

```bash
rg -n "MultiSNI|KeyAuth|HealthCheckTransition|ReferenceGrant" edgion-tests/src edgion-tests/integration/conf
```

Prefer copying a case with "same protocol, same validation method, same dependency type" rather than starting from scratch.

### 2. Modify YAML first, then Rust

The usual order is:

1. Write or modify YAML under `edgion-tests/integration/conf/<Resource>/<Item>/`
2. Update `edgion-tests/integration/conf/ports.json` if a new listener port is needed
3. If the YAML uses a new field, first confirm the corresponding CRD covers it
4. Then modify `edgion-tests/src/client/suites/...`

When a test fails, first determine "the configuration didn't take effect" vs "the assertion is wrong" — don't go straight to runtime code.

### 3. Reuse `test_server`; do not casually invent a new backend

For most HTTP, gRPC, WebSocket, TCP, UDP, ForwardAuth, OIDC, mirror, delay, and status-code scenarios, `test_server` is sufficient. Only extend `edgion-tests/src/server/test_server.rs` when existing endpoints or protocols truly do not cover your scenario.

The backend ports and endpoints already supported by `test_server` are in [references/test-server-capabilities.md](references/test-server-capabilities.md).

### 4. Suite registration touchpoints must be complete

When adding a new suite, you typically need to update at least:

- `edgion-tests/src/client/suites/<family>/...`
- `edgion-tests/src/client/suites/<family>/mod.rs`
- `edgion-tests/src/client/suites/mod.rs`
- `edgion-tests/src/client/test_client.rs`

If you only add an item under an existing family, the most commonly missed places in `test_client.rs` are:

- `resolve_suite()`
- `suite_to_port_key()`
- `add_suites_for_suite()`

### 5. Choose the validation method

The usual priority is:

1. Direct response assertions: status code, header, body, handshake success
2. Access log: plugin chain, condition match, internal fields, stage logs
3. Metrics: load balancing, hash, consistency, latency stats (**only existing counter/gauge — do not add a plugin-specific one**)
4. `resource_diff` / `config_load_validator`: whether the configuration was correctly accepted by controller/gateway

Rules of thumb:

- Plugin chains, conditional execution, header/body rewrite — access log is more reliable
- LB, retry, hash, consistency — metrics are more reliable
- `TLSRoute` / `EdgionTls` often require looking at handshake result, gateway log, and access log together

> **Metrics-assertion scope limit**: metrics assertions are only used for existing counter/gauge (e.g., `edgion_backend_requests_total`, `edgion_conn_ip_restriction_events_total`); **do not add a plugin-specific counter just to verify plugin behavior** (e.g., `edgion_rate_limit_rejected_total`). Response assertions + access log already verify auth, rate limit, rewrite, etc. correctly. See [`03-coding/observability/01-metrics.md`](../03-coding/observability/01-metrics.md) "Prohibitions: each plugin registers its own metrics".

### 6. Run a narrow scope

First run the narrowest command:

```bash
./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r <Resource> -i <Item>
```

If you need to repeatedly hit traffic or control phases yourself, after preserving the scene call:

```bash
./target/debug/test_client -g -r <Resource> -i <Item>
```

## Dynamic (hot reload) testing

Hot-reload tests are orchestrated by the Rust-side scenario framework and **run automatically by default** as the final step of the full integration test.

### How to trigger and inspect

```bash
# Default run: includes dynamic scenarios (all families + dynamic at the end)
./edgion-tests/integration/scripts/integration/run_integration.sh

# Running only -r Gateway: still includes dynamic (migrated from the legacy run_dynamic_tests behavior)
./edgion-tests/integration/scripts/integration/run_integration.sh -r Gateway

# Narrow run (-r set to non-Gateway, e.g. -r EdgionPlugins): skips dynamic for faster iteration

# List registered scenario ids
./target/debug/test_client --list-dynamic-scenarios

# Run a single scenario (after preserving the scene)
./target/debug/test_client -g --dynamic-scenario BasicAuthDynamic
```

The final step of the full `run_integration.sh` flow enumerates the output of `--list-dynamic-scenarios` and invokes `--dynamic-scenario` one by one; each appears as `Dyn_<id>` in `report.log`.

### Current scenario set

> Source of truth is `edgion-tests/src/client/dynamic/registry.rs` (`all()`); run `--list-dynamic-scenarios` for the live list. As of this writing it registers 10 scenarios:

| Scenario id | conf directory (`conf_path`) | Verifies |
|-------------|----------|---------|
| `GatewayDynamic` | `Gateway/DynamicTest` | Gateway/HTTPRoute config changes (migrated case) |
| `DslDynamic` | `EdgionPlugins/DslDynamic/DynamicTest` | DSL plugin source switch v1→v2 |
| `BasicAuthDynamic` | `EdgionPlugins/BasicAuth/DynamicTest` | `secretRefs` list shrinks; bob credentials removed → 401 |
| `BasicAuthDynamicGrouped` | `EdgionPlugins/BasicAuth/DynamicTest` | Grouped BasicAuth variant (`updates_grouped/`) |
| `RateLimitDynamic` | `EdgionPlugins/RateLimit/DynamicTest` | `onMissingKey` switched from Allow to Deny |
| `ObservedGenHighFreq` | `Status/ObservedGeneration/HighFrequency/DynamicTest` | 10 rapid Gateway spec changes; `observedGeneration` catches up to the final generation |
| `BtlspRefFlap` | `Status/BackendTLSPolicy/RefFlap/DynamicTest` | BackendTLSPolicy CA ConfigMap ref flaps good→missing→good; `Accepted`/`ResolvedRefs` track it |
| `PmdRefFlap` | `EdgionPlugins/EdgionConfigDataStatus/RefFlap/DynamicTest` | EdgionPlugins `serviceRegionRouteRef` flaps good→missing→good; `Accepted` tracks it |
| `RouteDelegationDynamic` | `HTTPRoute/Delegation/DynamicTest` | Delegation child rule moves /pay → /pay2; parent re-flattens and gateway hot-swaps routing |
| `SpBackendChange` | `HTTPRoute/Backend/SessionPersistenceBackendChange/DynamicTest` | SessionPersistence cookie re-routes to a healthy backend after backend-a is drained |
| `TcpEspReload` | `TCPRoute/StreamPlugins/DynamicTest` | Hot-reload of EdgionStreamPlugins allow-list on a TCPRoute listener (allow→deny→allow) |

### Where the framework lives

- Framework code: `edgion-tests/src/client/dynamic/`
  - `ctx.rs` — `ScenarioCtx`: composition operators `assert_phase`, `wait_phase`, `apply_dir`, `delete_resources`, `sleep` (the legacy `wait_resource_sync` method has been removed; the underlying `wait::wait_for_resource_sync` function and the `edgion-tests/src/validator/resource_diff` binary are kept as a setup-time sanity check)
  - `scenario.rs` — `Scenario` struct + `PhaseSuite` phase→suite factory mapping
  - `registry.rs` — explicit `all()` list (one line per new scenario)
  - `scenarios/<id>.rs` — one file per scenario; the N-phase state machine is glued together with `ScenarioCtx`
- Design doc: `docs/superpowers/specs/2026-04-25-plugin-dynamic-test-framework-design.md`
- Implementation plan: `docs/superpowers/plans/2026-04-25-plugin-dynamic-test-framework.md`

### Adding a dynamic scenario

1. Write `initial/` + `updates/` YAML under `edgion-tests/integration/conf/<Resource>/<Item>/DynamicTest/` (`load_conf.sh` automatically skips `updates/` and `delete/`)
2. Write two phase suites in `edgion-tests/src/client/suites/<family>/dynamic.rs` (`<Name>InitialTestSuite` / `<Name>UpdateTestSuite`); re-export them via the family `mod.rs` to `suites::*`
3. Add branches to the three dispatch tables in `test_client.rs`: `resolve_suite` (for aliases if needed), `suite_to_port_key`, `add_suites_for_suite` (selects a suite by `--phase`)
4. Write the scenario in `edgion-tests/src/client/dynamic/scenarios/<id>.rs`; the `run` function chains the N phases in order: `assert_phase` (initial) → `apply_dir` / `delete_resources` (changes) → `wait_phase` (with timeout, until the final-state business assertions all pass). **Do not** rely on `sleep` as a fallback and **do not** use `wait_resource_sync` — the former is unreliable, and the latter is no longer exposed on ScenarioCtx
5. Add `pub mod <id>;` to `dynamic/scenarios/mod.rs`
6. Append `crate::dynamic::scenarios::<id>::SCENARIO` to `all()` in `dynamic/registry.rs`

After that `--list-dynamic-scenarios` lists it automatically, and `run_integration.sh` (default) includes it under the full run or the `-r Gateway` path.

### Manual phase-by-phase debugging

A scenario internally schedules suites via `ctx.assert_phase(scenario, "initial"|"update")`. To bypass the orchestration manually, the legacy path is still available (after preserving the scene):

```bash
# Run only the initial-phase assertions
./target/debug/test_client -g -r <Resource> -i <Item> --phase initial
# After manually applying updates/, run the update-phase assertions
./target/debug/edgion-cli --server http://127.0.0.1:12101 \
    apply -f edgion-tests/integration/conf/<Resource>/<Item>/DynamicTest/updates/
sleep 3
./target/debug/test_client -g -r <Resource> -i <Item> --phase update
```

The `--phase` argument is still effective (each dynamic dispatch routes to the corresponding suite by phase) and is the common entry point for investigating a phase's behavior inside a scenario.

> Note: scenarios no longer use a fixed `sleep` for synchronization — see the `wait_phase` operator in the ScenarioCtx list above. For manual debugging, `sleep` is still acceptable since a human at the terminal can read the log to decide on the timeout.

## Common investigation order

1. First check `${WORK_DIR}/report.log` and `${WORK_DIR}/test_logs/<case>.log`
2. Then check `${WORK_DIR}/logs/controller.log` and `${WORK_DIR}/logs/gateway.log`
3. For plugin or routing-match issues, check the access log / `tls_access.log`
4. If you suspect the configuration was not loaded, use `edgion-cli` or the admin API to query the controller/gateway's current resources
5. If you suspect the test backend behaves wrong, go back to `test_server` ports and endpoint capabilities

For detailed commands and log entry points see [../06-tracing/00-debugging.md](../06-tracing/00-debugging.md).

## Authentication behavior in integration tests

Authentication is **token-only** (Istio-style pre-shared bearer token). In integration tests it is handled by the framework automatically — no manual action is required:

- Integration tests set `EDGION_ADMIN_TOKEN` explicitly before starting the controller (see `test_controller_no_auth.sh` for the positive-case pattern)
- When `load_conf.sh` invokes `edgion-cli`, `resolve_token()` picks up `EDGION_TOKEN` / `~/.edgion/token` and attaches the bearer token automatically (no login exchange)
- `start_all_with_conf.sh` exports `EDGION_WORK_DIR` and the admin/cli token env vars so the token path is reachable
- CI / test environments require no manual `edgion-cli login` step

### Controller no-token regression test

`edgion-tests/integration/scripts/integration/test_controller_no_auth.sh` is a self-contained integration-layer regression
verifying that, with no token configured, the Controller: starts successfully, returns 200 for `/health` and
`/api/v1/auth/status`, returns 503 (fail-close) for business paths, and the startup log contains a no-token-configured WARN.
A second positive case exports `EDGION_ADMIN_TOKEN` and asserts an authenticated request succeeds. It is included in the
preflight of `run_integration.sh` as a structural regression and does not depend on the Gateway/test_server environment.

## Gateway / Controller reuse principle

> Premise of integration-test design: **the same kind of tests reuse the same Gateway + Controller**. New suites are attached to the main `run_integration.sh` processes by default, and **must not** spawn their own `edgion-gateway` / `edgion-controller`.

### Main flow

`run_integration.sh` → `start_all_with_conf.sh` starts 1 Gateway + 1 Controller. All family suites (HTTPRoute / GRPCRoute / TCPRoute / UDPRoute / TLSRoute / EdgionPlugins / EdgionTls / Gateway / Status / LinkSys / dynamic scenarios) share the same processes; configuration changes are made online via `TestContext::apply_yaml()` / `delete_resource()` to drive Controller-to-Gateway sync without restarts.

### Standalone-topology exceptions (legitimate)

Only when a **completely different process-level configuration** is needed should a standalone start be used; each standalone scenario still starts only one set:

| Script | Reason for standalone topology |
|---|---|
| `test_file_watcher.sh` | Controller FileSystem source (mutually exclusive with K8s mode) |
| `test_resource_abnormal_metrics.sh` | Slow-test-only Controller `tick_secs=30` |
| `test_center_no_auth.sh` | Center fail-close validation without auth (no Controller/Gateway) |
| `run_center_test.sh` | 1 Center + 2 Controllers federation validation |
| `run_center_mtls_test.sh` | Center federation mTLS + SPIFFE peer-identity binding (cert SAN ↔ controller_id), wrong-identity rejection, plaintext fail-close |
| `run_conf_sync_mtls_test.sh` | Controller↔Gateway mTLS channel |
| `run_acme_test.sh` | Standalone ACME topology |

A new standalone script must include a header comment explaining "why the main environment cannot be reused"; otherwise the PR cannot be merged.

### Antipatterns (signals that block reviewer approval)

When a new suite / new script contains any of the following, the reviewer must block the merge and ask the author to either reuse the main process or be added to the exception list above:

- `Command::new("edgion-gateway")` / `Command::new("./edgion-gateway")` / `spawn_gateway`-style calls
- A new script under `edgion-tests/src/client/` or `edgion-tests/integration/scripts/` (not in the exception list) calling `start_all` / `start_all_with_conf`
- Repeatedly killing / restarting `edgion-gateway` within the same script (unless the test is specifically about Gateway restart behavior — even then only one restart cycle is allowed)

Reasonable `Command::new` examples (do not block): invoking external tools like `openssl`, `docker`, `curl` to validate client-side protocols — they don't count as "starting Gateway".

### CI safety net

After startup, `run_integration.sh` does a self-check `pgrep -c edgion-gateway`; if it detects ≥ 2 `edgion-gateway` processes during the run it fails fast and exits (to prevent silent violations). If your new suite triggers this check, first see whether cleanup is missing or `Command::new("edgion-gateway")` is misused.

## Common pitfalls

- YAML written under `edgion-tests/integration/conf/` but not registered in `test_client.rs` — the case never runs
- New item missing in `suite_to_port_key()` — traffic hits the wrong listener
- `--gateway` missing — a suite that should go through the gateway errors out
- Reusing old test certificates — SAN mismatch
- Runtime-generated Secrets are in `${WORK_DIR}/generated-secrets/` but you only inspect template YAML
- Assuming a gateway bug when it's actually a `test_server` path or port mismatch
- Modifying `LinkSys` or K8s scenarios directly without switching to the corresponding skill doc

## When to read other docs

- For specific suite/resource/item mappings: [references/integration-suite-map.md](references/integration-suite-map.md)
- To verify `test_server` ports, endpoints, OIDC/auth, mirror capabilities: [references/test-server-capabilities.md](references/test-server-capabilities.md)
- To preserve the scene for manual investigation: [../06-tracing/00-debugging.md](../06-tracing/00-debugging.md)
- To run the K8s version: [02-k8s-integration-testing.md](02-k8s-integration-testing.md)
- To run LinkSys: [03-link-sys-testing.md](03-link-sys-testing.md)
