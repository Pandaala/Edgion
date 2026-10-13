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

## Bash portability: never `((x++))` under `set -e`

In any script with `set -e`, never use bare `((x++))` to bump a counter — use `x=$((x + 1))`. `((expr))` returns exit status **1** when `expr` evaluates to **0**, and `x++` is a *post*-increment that evaluates to the OLD value, so the first increment of a counter initialised to 0 returns 1 and `set -e` silently kills the script — with no error output and a healthy-looking stack. This is bash-version-dependent: bash 3.2 (shipped by macOS) does not enforce `-e` for arithmetic commands, so the bug is invisible on dev Macs and fires immediately on bash 5 (Debian bookworm / GitHub `ubuntu-latest`). Pre-increment `((++x))` happens to work but relies on the same subtlety — prefer the assignment, which has no failure mode. The integration preflight runs `cicd/checks/check_bash_set_e_increment.sh` to enforce this rule; its focused regression test is `cicd/checks/tests/check_bash_set_e_increment_test.sh`.

## Shell portability baseline

Integration scripts must run with both macOS Bash 3.2 and Linux Bash 5. Keep syntax within the Bash 3.2 feature set: indexed arrays are available, but associative arrays, `mapfile`/`readarray`, `declare -g`, `${var@Q}`, and `printf '%(...)T'` are not. Prefer the common BSD/GNU utility subset; when a command differs (`sed -i`, `date`, `readlink`, or `base64` flags), detect the capability and provide both forms instead of branching on an assumed OS name.

Do not depend on ambient interactive variables such as `USER`; query the required fact directly (`id -u`). Preserve data semantics across tool implementations—for example, do not parse a 64-bit JSON identifier through a representation that rounds it. For asynchronous services, poll an observable readiness or state transition with a bounded timeout instead of increasing a fixed `sleep`. Validation should combine `bash -n` for every script, focused static guards, and real execution on both macOS and a Linux Bash 5 container.

Under `set -e`, also audit command substitutions containing pipelines. An assignment inherits the substitution's status, and commands such as `grep -c` return 1 for zero matches even while printing `0`. If zero is valid assertion input, contain the status inside the substitution (for example, `count=$(... | grep -c . || true)`) so the following assertion can emit a useful failure.

Shared cross-platform operations live in `edgion-tests/integration/scripts/utils/shell_compat.sh`. It detects Darwin or Linux once, exports `EDGION_SHELL_PLATFORM`, and routes OS-specific behavior through `edgion_platform_call`; callers must not repeat `uname` branches. Use `edgion_count_nonempty_lines`, `edgion_json_integer`, `edgion_live_pids_by_name`, `edgion_missing_loopback_aliases`, and `edgion_wait_until` instead of reimplementing their edge cases. Prefer an OS-neutral implementation when the command contract is truly common, and add paired `_edgion_<operation>_darwin` / `_edgion_<operation>_linux` adapters when BSD and GNU behavior differs.

The standard `run_all_integration.sh` and `run_integration.sh` entry points source this library and export it through `BASH_ENV`, so every descendant Bash script receives the helpers automatically. A standalone script that must also work when invoked directly should source the library explicitly. Run `cicd/checks/tests/shell_compat_test.sh` after changing the shared behavior.

## Fixed ports in isolated scenarios

Standalone scenarios with fixed host ports load them from
`edgion-tests/integration/scripts/utils/isolated_ports.sh` and check their required
variables with `isolated_require_ports` before starting a process. Register a new
fixed port there; do not duplicate its numeric value in the scenario script.
`run_integration.sh` calls `check_isolated_ports.py` before building or starting
dependencies. The check rejects duplicate assignments and probes each selected
TCP port using its registered bind address and socket reuse policy. This avoids
flagging unrelated outbound connections that do not block the real listener.
Docker-only scenarios are probed after Docker
availability is known, before those scenarios start. OIDC egress and UDP process
shutdown choose ports dynamically and do not need fixed assignments.

`conf/ports.json` has a different purpose: it maps shared Gateway test suites to
their listeners. Keep isolated process ports out of that file.

## Preflight quality gates

Integration is the project's key control point, so `run_integration.sh` enforces four repo-wide quality gates near the top of `main()`, in order. **Any failure aborts with `exit 1` before any build or test runs**, and they run on every invocation — including `--no-prepare` (only the build step is gated by prepare, not these gates). The isolated-port preflight runs before these gates for a full sweep.

| Gate | Entry point | Enforces |
|------|-------------|----------|
| English-Only Lint | `cicd/checks/check_english_only.sh` | no non-English letter scripts outside `docs/` and `tasks/` |
| Agent-doc Validation | `python3 cicd/checks/validate_agent_docs.py` | `AGENTS.md` / `skills/` links, frontmatter, indexes, stale `src/` refs |
| Types Purity | `cicd/checks/check_types_purity.sh` | `edgion-resources` stays a clean leaf crate (no `crate::core::` edges) |
| Bash Portability | `cicd/checks/check_bash_set_e_increment.sh` + `cicd/checks/tests/shell_compat_test.sh` | no bare arithmetic increment/decrement can trip `set -e`; shared OS adapters work on the current platform and propagate through `BASH_ENV` |

These are a subset of repository quality gates, not a replacement for pre-commit or all
GitHub workflows. The runner does not run fmt, workspace check/clippy, SSA-force,
Gateway no-tracing, or every independent CI guard. To reproduce one of these four
preflights, run its `cicd/checks/*` entry point directly.

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

# Bound parallel standalone scenarios in a default full sweep (default: 4)
./edgion-tests/integration/scripts/integration/run_integration.sh --jobs 2

# Keep the scene after the run for manual investigation
./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare --keep-alive -r Gateway -i StreamPlugins
```

### Gateway capability gating

The runner builds and starts `edgion-gateway` against Edgion's pinned Pingora
fork and probes the binary through `--print-build-capabilities`; it does not
infer capability support from an environment variable or filename.

Capability-dependent suites and dynamic scenarios are registered in
`edgion-tests/integration/capabilities.tsv` (currently the `body-buffering`
capability, which every build now reports). An exact unsupported
`-r <Resource> -i <Item>` request fails instead of succeeding as a no-op.

TLSRoute Terminate positive tests have a separate product-support boundary:
the Controller currently rejects explicit TLS/Terminate listeners and does not
declare `TLSRouteModeTerminate`. Pure Terminate suites report SKIP in sweeps
(explicit requests fail); mixed suites retain supported protocol checks and
record DEFERRED subcases in the final report. The status suite verifies
`Accepted=False/UnsupportedValue` and empty `supportedKinds`. Positive checks
remain disabled while the feature is unsupported. This is not a Gateway build
capability and must not be inferred from the presence of termination proxy code.

### Run every integration test in one shot

`run_integration.sh` covers suites sharing one Controller/Gateway topology. The in-repo
orchestrator also runs the declared standalone LinkSys, ACME, CLI, and FileWatcher jobs.
It does not cover tests owned by the separate EdgionCenter repository.

```bash
# Run every job in fast mode, continue on failure, unified report at the end
./edgion-tests/integration/scripts/integration/run_all_integration.sh

# Skip the top-level build; fail fast if required artifacts are absent
./edgion-tests/integration/scripts/integration/run_all_integration.sh --no-prepare

# Propagate --full-test to the main runner (LdapAuth, slow timeouts, etc.)
./edgion-tests/integration/scripts/integration/run_all_integration.sh --full-test

# Limit the run
./edgion-tests/integration/scripts/integration/run_all_integration.sh --only main,linksys_etcd
./edgion-tests/integration/scripts/integration/run_all_integration.sh --skip acme,file_watcher

# Override the default one-hour job and six-hour whole-run deadlines
./edgion-tests/integration/scripts/integration/run_all_integration.sh \
  --job-timeout-secs 3600 --run-timeout-secs 21600
```

Each job's stdout/stderr is captured under the run directory with a machine-readable
summary. Current names are `main`, `linksys_etcd`, `linksys_es`, `linksys_otlp`,
`linksys_kafka`, `acme`, `cli`, and `file_watcher`. Treat
`run_all_integration.sh::JOB_NAMES` as authoritative.

The aggregate runner gives every complete job a deadline and also enforces one whole-run budget,
including the top-level build. The defaults are 3600 seconds per job and 21600 seconds overall;
override them with the CLI flags above or `EDGION_RUN_ALL_JOB_TIMEOUT_SECONDS` /
`EDGION_RUN_ALL_TIMEOUT_SECONDS`. A timeout is reported separately as `TIMEOUT`; the owned process
group receives TERM, then KILL after `EDGION_DEADLINE_GRACE_SECONDS` (default 10), and the remaining
jobs still reach the aggregate summary.

`prepare.sh` is the canonical build phase. In addition to product and helper binaries, it builds
`cache_residue` and precompiles the ignored real-Redis integration tests
(`credential_reservation_redis`, `token_bucket_partial_refill_redis`), copying each hashed Cargo
test executable to `target/debug/<test name>` (or the coverage target equivalent). Add a new
ignored test to the `for ignored_test in ...` list in `prepare.sh` and to the artifact list in
`integration/scripts/README.md`; a guard test in `test_client.rs` compares the two. Non-coverage
preparation normalizes `CARGO_TARGET_DIR` to the project-local
`target/` directory so every child resolves the same paths. Execution phases invoke those stable
paths directly. `--no-prepare` is therefore
strictly build-free: a missing artifact is an immediate infrastructure failure, never a late Cargo
invocation. The aggregate ACME child uses `--no-build`; a directly invoked ACME script may build
the canonical artifact set through `prepare.sh`, but only before starting Pebble or any test fixture.

## Memorize the actual directory layout

- `edgion-tests/integration/conf/`: test YAML, organized by resource/item
- `edgion-tests/src/client/`: Rust test client and suite implementations
- `edgion-tests/src/server/test_server.rs`: local HTTP/gRPC/WebSocket/TCP/UDP test backend
- `edgion-tests/src/server/transport/`: raw HTTP/1.1 + HTTP/2 transport-shape backends and the transport event recorder
- `edgion-tests/src/validator/`: `resource_diff`, `config_load_validator`
- `edgion-tests/integration/scripts/integration/run_integration.sh`: main entry (shared controller/gateway suites)
- `edgion-tests/integration/scripts/integration/run_all_integration.sh`: in-repo orchestrator for the stable job list above
- `edgion-tests/integration/scripts/utils/`: `prepare.sh`, `start_all_with_conf.sh`, `load_conf.sh`, `kill_all.sh`
- `edgion-tests/integration/scripts/gen_certs/`: test certificate generation scripts
- `tmp/integration_testing/testing_<timestamp>/`: this run's logs, PIDs, generated Secrets, and report directory

Do not search for code or cert scripts using older test-directory conventions. Current code lives in `edgion-tests/src/`; cert scripts are in `edgion-tests/integration/scripts/gen_certs/`.

## Run flow

The actual responsibilities of `run_integration.sh` are:

1. Call `prepare.sh` once to build `edgion-controller`, `edgion-gateway`, `edgion-cli`, `test_server`, `test_client`, `test_client_direct`, `resource_diff`, and `config_load_validator` in one Cargo invocation
2. Call `start_all_with_conf.sh` to clean the scene and create `tmp/integration_testing/testing_<timestamp>/`
3. Generate runtime Secrets under `${WORK_DIR}/generated-secrets/` and load them once after the suite YAML
4. Start `test_server`, controller, gateway, and wait for health checks
5. First load `edgion-tests/integration/conf/base/`, then load the configuration of the target suite, and finally load runtime-generated Secrets
6. Use `resource_diff` to verify the controller↔gateway sync state
7. Call `test_client -g -r <Resource> -i <Item>` to run the tests
8. Clean up by default; with `--keep-alive` the processes, logs, and working directory are kept

A few key facts:

- `--full-test` additionally includes slow tests and requires Docker to be available. In a
  fast run, the consistently longer-than-three-minute `HTTPRoute_Transport` suite and
  `Dyn_EncryptedActiveHealthChecks` scenario are reported as skipped. A targeted
  `-r HTTPRoute -i Transport` run also requires `--full-test` and otherwise skips before
  preparing the topology.
- A targeted `EdgionPlugins/ExtProc` run with `--full-test` builds and starts the pinned independent Go processor,
  renders its full-only resources into the work directory, and forwards the flag to `test_client`.
  The added cases cover official Envoy v3 protobuf interoperability, header and body mutation,
  `ImmediateResponse`, clean terminal half-close, and a wall-clock long-idle stream. The default
  targeted run remains Go-free and fast.
- `ExtProcTlsPolicyRecovery` is a full-tier dynamic scenario using that same
  processor. Full ExtProc targets and full dynamic sweeps include it; a full
  resource-wide Gateway run also prepares the fixture. Fast dynamic sweeps
  report an explicit skip. It observes unsupported local/inherited TLS policy
  rejection, zero TCP/RPC deltas, and recovery without restarting the topology.
- `run_integration.sh` first checks `ulimit -n` and tries to raise it to `65535` if insufficient
- `start_all_with_conf.sh` exports `EDGION_WORK_DIR` and `EDGION_GENERATED_SECRET_DIR`
- The default full sweep runs its isolated standalone scenarios through a bounded parallel queue.
  They use separate ports and work directories; `--jobs N` or `EDGION_INTEGRATION_JOBS=N`
  controls the limit. Each scenario defaults to a 900-second deadline and the entire isolated phase
  to 1800 seconds; configure them with `--isolated-timeout-secs`,
  `--isolated-phase-timeout-secs`, or their `EDGION_ISOLATED_*_TIMEOUT_SECONDS` environment forms.
  Shared-topology suites remain serial because several mutate resources or assert cumulative
  access-log and metric state.
- Each suite's sorted YAML list is submitted with repeated `edgion-cli apply -f` flags. This reuses
  one CLI runtime and HTTP client while preserving the existing resource submission order.
- Base configuration is loaded once, and inferred dependency suites are de-duplicated in first-seen
  order. Any batch failure aborts startup before tests run; the runner no longer continues with a
  partially loaded topology. Configuration loading checks both the CLI batch and
  its log sink; failure identifies the suite and `logs/load_config.log` while
  preserving the CLI's per-file diagnostics in the output. Run
  `bash cicd/checks/tests/integration_config_load_test.sh` to check this failure
  boundary without starting services.
- conf_sync mTLS certs live at `${WORK_DIR}/certs/grpc/{primary,secondary}/`, generated in place
  by `conf_sync_distribute_certs "$WORK_DIR"` (the script that creates a work directory owns its
  certs — standalone scripts generate into their own `WORK_DIR`/`mktemp` dir; nothing is written
  to the repo root or any shared cache; `prepare.sh` is build-only)
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

The Rust runner treats suite lifecycle as part of the test result. A failed `setup` or `teardown`
hook is an infrastructure failure even when every executed case passed. A selection that executes
zero cases also fails by default; callers must use `TestRunner::allow_empty_selection` explicitly
when an empty result is the intended assertion. Console and JSON reports expose these errors, and
the test-client process exits non-zero for them.

Gateway-mode listener selection is fail-closed. `ports.json` must load, the selected suite key must
exist, and its primary protocol field (`http`, `grpc`, `https`, `grpc_tls`, `tcp`, `tls`, or `udp`)
must be present. A suite-level listener contract must resolve its declared protocol or the suite is
skipped and the run fails as an infrastructure error. Only explicitly classified
control-plane/status suites may run without a data-plane listener mapping. Direct mode continues to
use its CLI ports. Aggregate selections attach a key and protocol kind to every child suite, so
mixed HTTP, gRPC, TLS, TCP, and UDP children do not inherit one listener. K8s creates the
`edgion-test-ports` ConfigMap from this canonical file and mounts it as `conf/ports.json`; do not
embed a second registry in Kubernetes YAML.

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

For **transport-level** needs — an upstream that responds before the request body is complete, half-closes mid-request, resets at a chosen byte offset, never responds, or answers with deliberately malformed framing — do not add another mock. `test_server` already exposes two raw listeners for exactly this (`--transport-h1-port 30030`, `--transport-h2-port 30031`), steered per request by `x-up-*` headers, plus a transport event recorder at `/transport/query/{trace_id}` that reports what the gateway actually put on the wire toward the upstream (including upstream connection identity and reuse, which no echoed response header can show). See the "Transport-shape backends" section of the capability reference.

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

Dynamic scenarios have an additional fail-closed inventory contract. Add the scenario to
`dynamic::registry::all`, update `EXPECTED_DYNAMIC_SCENARIO_IDS`, declare its listener contract, and add
any required Gateway capability entry. `test_client --list-dynamic-scenarios` validates all of these before
printing; a command failure or empty list is an integration infrastructure failure, never “no scenarios”.

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

For cumulative metrics, take a baseline before generating test traffic and assert the post-test delta.
An LB distribution test must also fail on request errors and unexpected status codes, use sequential
requests when claiming strict RoundRobin order, and assert the exact endpoint set, request count, and
documented balance tolerance. A descriptive `is_balanced` value in a success message is not an assertion.

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

Hot-reload tests are orchestrated by the Rust-side scenario framework and **run automatically by default** as the final step of the full integration test. The slow `Dyn_WafDynamic` and `Dyn_EncryptedActiveHealthChecks` scenarios require `--full-test`. This also applies to the targeted `-r Gateway -i WafDynamic` runner path. A fast run reports these scenarios as skipped. Ordinary static WAF tests and `Dyn_WafExpensiveChecks` remain in the default sweep.

### How to trigger and inspect

```bash
# Default run: includes dynamic scenarios (all families + dynamic at the end)
./edgion-tests/integration/scripts/integration/run_integration.sh

# Include slow suites and hot-reload matrices (Transport, encrypted health checks, WAF)
./edgion-tests/integration/scripts/integration/run_integration.sh --full-test

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

> Source of truth is `edgion-tests/src/client/dynamic/registry.rs` (`all()`); run
> `--list-dynamic-scenarios` for the validated live list. As of this writing it registers 29 scenarios. The
> table below highlights the common and protocol-lifecycle scenarios rather than duplicating the
> complete registry.

| Scenario id | conf directory (`conf_path`) | Verifies |
|-------------|----------|---------|
| `GatewayDynamic` | `Gateway/DynamicTest` | Gateway/HTTPRoute config changes (migrated case) |
| `AiBackendPolicyIntegration` | `EdgionBackend/PolicyIntegration/DynamicTest` | Kubernetes-backed AI backend custom CA, SNI/SAN, mTLS, Secret rotation, processor status, multi-target traffic policy publication, hot reload, stable in-flight capture, and stream termination boundaries |
| `DslDynamic` | `EdgionPlugins/DslDynamic/DynamicTest` | DSL plugin source switch v1→v2 |
| `BasicAuthDynamic` | `EdgionPlugins/BasicAuth/DynamicTest` | `secretRefs` list shrinks; bob credentials removed → 401 |
| `BasicAuthDynamicGrouped` | `EdgionPlugins/BasicAuth/DynamicTest` | Grouped BasicAuth variant (`updates_grouped/`) |
| `RateLimitLocalDynamic` | `EdgionPlugins/RateLimitLocal/DynamicTest` | `onKeyMiss` switched from Allow to Deny |
| `RequestAccessPolicyHmacDynamic` | `EdgionPlugins/RequestAccessPolicy/DynamicTest` | Nested HMAC credentials rotate through a Secret update; old signatures stop matching and new signatures start matching without a restart |
| `CsrfSecretRotation` | `EdgionPlugins/Csrf/DynamicTest` | CSRF signing-key Secret rotation rejects the pre-rotation token, accepts a newly issued token, fail-closes on deletion, and recovers after restore |
| `ObservedGenHighFreq` | `Status/ObservedGeneration/HighFrequency/DynamicTest` | 10 rapid Gateway spec changes; `observedGeneration` catches up to the final generation |
| `BtlspRefFlap` | `Status/BackendTLSPolicy/RefFlap/DynamicTest` | BackendTLSPolicy CA ConfigMap ref flaps good→missing→good; `Accepted`/`ResolvedRefs` track it |
| `PmdRefFlap` | `EdgionPlugins/EdgionConfigDataStatus/RefFlap/DynamicTest` | EdgionPlugins `overrideRef` flaps good→missing→good; `ResolvedRefs` tracks it (`False`/`InvalidEdgionConfigDataRef` while missing) |
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

1. Write phase YAML under `edgion-tests/integration/conf/<Resource>/<Item>/DynamicTest/`. Remember that the static loader may preload dynamic fixture files; use unique resource names and make the scenario clean up and poll for absence before applying its initial generation.
2. Define phase suites either in `edgion-tests/src/client/suites/<family>/dynamic.rs` for reusable traffic suites, or directly in the scenario for focused Controller/Gateway state assertions.
3. Add `test_client.rs` dispatch entries only when the phase suites must also be selectable through `-r/-i --phase`; a self-contained registered scenario does not need those aliases.
4. Write the scenario in `edgion-tests/src/client/dynamic/scenarios/<id>.rs`; the `run` function chains the N phases with `apply_dir` / `delete_resources` and `wait_phase` (with timeout, until the final-state business assertions all pass). Always clean up on success and failure. **Do not** rely on `sleep` as a fallback and **do not** use `wait_resource_sync` — the former is unreliable, and the latter is no longer exposed on ScenarioCtx
5. Add `pub mod <id>;` to `dynamic/scenarios/mod.rs`
6. Append `crate::dynamic::scenarios::<id>::SCENARIO` to `all()` in `dynamic/registry.rs`
7. Add the exact ID to `EXPECTED_DYNAMIC_SCENARIO_IDS` and declare its `dynamic_port_contract`; add a
   `dynamic` row to `integration/capabilities.tsv` if it requires a build capability

After all registration contracts agree, `--list-dynamic-scenarios` lists it, and `run_integration.sh`
(default) includes it under the full run or the `-r Gateway` path. Any drift fails enumeration.

### Manual phase-by-phase debugging

A scenario internally schedules suites via `ctx.assert_phase(scenario, "initial"|"update")`. For low-level debugging, you can bypass orchestration and invoke the current phase flags directly after preserving the scene:

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

`report.log` is a run-level report: it includes ordinary preflights, isolated scenarios, the main
suites, and the final metrics, unknown-field, requeue-loop, and cache-residue gates. A pending report
is created under `tmp/integration_testing/` before preflights and moved to the shared work directory
after startup. `Status: INCOMPLETE` means the process stopped before normal finalization; its
`Run_Aborted` entry identifies the last phase and exit code. Only `Status: PASS` is a completed
successful run. The report is atomically replaced after each recorded result, so readers never
observe a partially written summary. If an isolated scenario fails before the shared stack creates
its work directory, an incomplete report is also stored beside that scenario's logs.

Mandatory final gates fail closed when their prerequisites are unavailable. Metrics and diagnostics
endpoints must be reachable, the Controller admin token must exist, `controller.log` must be
readable, and the cache-residue validator must be executable. Only explicit policies produce a
recorded skip: `--skip-residue-check` / `SKIP_RESIDUE_CHECK=true` disables the gate, while
`RESIDUE_MODE=warn` records detected residue without failing the run.

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
| `run_conf_sync_mtls_test.sh` | Controller↔Gateway mTLS channel |
| `run_acme_test.sh` | Standalone ACME topology |
| `run_otlp_test.sh` | Immutable Gateway process YAML selects shared OTLP primary/fallback outputs; isolated owned collectors test hot LinkSys generations |
| `test_oidc_egress.sh` | Gateway-startup proxy environment and process-local multi-address DNS injection for both OIDC client branches |
| `test_debug_access_log_wire.sh` | Replace/Wrap masking is Gateway-startup-global; two serial isolated modes verify response headers and plugin hot reload |
| `test_http_process_shutdown.py` | SIGTERM, worker scheduling, and grace/drain limits require owned processes; verifies idle keepalive closure and active SSE/gRPC completion on two listeners |
| `benchmark_http_shutdown.py` | Manual notifier A/B comparison uses separately built Gateway binaries with identical worker settings; each sample owns and drains its processes |

A new standalone script must include a header comment explaining "why the main environment cannot be reused"; otherwise the PR cannot be merged.

Run the debug access-log scenario directly with
`bash edgion-tests/integration/scripts/integration/test_debug_access_log_wire.sh`.
It uses prebuilt Controller/Gateway/test_server binaries and generates its own
fixtures; it is not registered in the frozen existing runners. See the
[wire/lifecycle coverage contract](../04-review/observability/regression-guards/debug-access-log-wire-and-lifecycle.md)
for the build command, timeout options, evidence, and limits.

The OTLP runner uses its isolated `otlp_test_assertions.py` state/deadline driver
(`wait_phase`) rather than registering in the default Rust dynamic registry:
that shared topology cannot change startup-global log output references. It starts
one Gateway/Controller set, owns exact child PIDs and a dedicated compose project,
and applies LinkSys/Secret updates through the ordinary Controller API. Run
`bash edgion-tests/integration/scripts/integration/link_sys/run_otlp_test.sh`.
Low-level production transport tests run separately with
`cargo test -p edgion-gateway --features test-util --test otlp_transport`;
the test-only ResolverSpec injection is not an operator configuration field.

Center/federation topology tests are maintained in the separate EdgionCenter repository.

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

### Admission-negative status fixtures and startup failures

Schema-invalid Gateway API documents belong outside `integration/conf/`, because
shared startup requires every preloaded resource to pass Admin admission. The
BackendTLSPolicy CA-selection negative fixtures live in
`integration/fixtures/status-admission/`; their named tests send authenticated
Admin requests and assert rejection and nonpersistence. TLS mode status fixtures include certificate references and distinct listener
hostnames. Local finite admission currently accepts HTTPS/Passthrough, whose
Controller status is UnsupportedValue; the native CRD CEL rejects that combination.
The HTTPS test therefore asserts local status or an actual rejected Admin request
forwarded to Kubernetes, rather than reporting an unconditional pass. This does
not claim full local/native CEL parity. Other unsupported implementation features
still assert Controller status.
These probes test Controller Admin behavior, not native Kubernetes conformance.

Each shared startup writes its attempted work directory to an invocation-owned
handoff file before starting services. The runner uses that directory in failure
reports and recorded-PID cleanup even when startup fails; `.current` continues to
identify the last successfully started stack. Runner cleanup uses
`kill_all.sh --work-dir <directory>` and verifies recorded executable identities;
it never falls back to the old `.current` on a failed new startup. The no-argument
manual cleanup command retains its existing behavior.

The existing `ConditionRefsDynamic` scenario also covers retained IpList consumers:
one shared list changes ordinary entry gating and RAP primary membership through
content update, deletion and restoration without plugin updates. It retains the
existing Secret phases and restores both values on success/failure. Use the shared
topology and one resource per filesystem YAML fixture.

ARCH-028 B3 extends ConditionRefsDynamic on its existing topology with HTTP and global RealIp routes. It checks union content changes, missing required lists, global visibility flips, deletion and recovery using echoed canonical remote-IP headers. Retain its original Secret and ordinary/RAP IP phases, and restore every changed list in the finalizer. RealIp fixture lists belong to their existing owning namespaces; global trust lists use explicit Cluster-visible references. Native integration migration does not establish live Kubernetes or full conformance execution.
