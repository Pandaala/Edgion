---
name: 05-testing
description: Testing skill for Edgion. Use when writing, running, or extending unit tests, integration tests, K8s tests, LinkSys tests, or configuration sync validation workflows.
---

# 05 Testing

> Edgion test system. Goal: **maximize meaningful coverage without padding** — write tests worth keeping (critical paths, branches, risk boundaries, with real assertions) and push coverage as high as that allows; do not add low-value tests just to hit a line-coverage number.
> Split into **standard tests** (critical-path & risk-boundary coverage) and **dedicated tests** (specific-scenario validation).

## File list

### Part 1 — Standard tests (critical-path & risk-boundary coverage)

| File | Topic | Status |
|------|------|------|
| [00-unit-testing.md](00-unit-testing.md) | Unit testing: inline test module, how to run, writing rules, division of labor with integration tests | Complete |
| [01-integration-testing.md](01-integration-testing.md) | Integration testing: architecture, run flow, steps to add a test, **dynamic (hot reload) test scenario framework** | Refactored |
| [02-k8s-integration-testing.md](02-k8s-integration-testing.md) | K8s integration testing: differences from local testing, modification checklist, execution phases | Complete |
| [05-snapshot-testing.md](05-snapshot-testing.md) | Snapshot testing: when to use / refuse, expect-test workflow, volatile-field escape hatches | Complete |

### Part 2 — Dedicated tests (specific-scenario validation)

| File | Topic | Status |
|------|------|------|
| [03-link-sys-testing.md](03-link-sys-testing.md) | LinkSys integration tests: bash test flow, Admin API validation, Docker Compose | Complete |
| [04-conf-sync-leak-testing.md](04-conf-sync-leak-testing.md) | Config-sync leak detection: basic loop test + advanced boundary scenarios + **mTLS channel test** | Complete |
| [02-cache-residue-detection.md](02-cache-residue-detection.md) | Cache residue detection: wipe-all + store-stats leak detection, adding a new store / ignore-path, debugging leak reports | Complete |

## Quick start

### Run unit tests
```bash
cargo test --all
```

### Run integration tests
```bash
# Main runner — every suite that shares the single controller/gateway topology
# (HTTPRoute, GRPCRoute, EdgionPlugins, EdgionTls, Status, LinkSys/Status,
# LinkSys/Redis, dynamic scenarios, …)
./examples/test/scripts/integration/run_integration.sh

# Top-level orchestrator — main runner PLUS every standalone integration script
# (Etcd / Elasticsearch LinkSys, ACME, Center watch-sync, edgion-cli E2E,
# FileWatcher). Sequential; continues on failure; unified report at the end.
./examples/test/scripts/integration/run_all_integration.sh
```

### Run dynamic (hot reload) tests
Dynamic scenarios run by default as the final step of the full `run_integration.sh` (without `-r`) and the `-r Gateway` path; narrow paths (`-r EdgionPlugins -i ...` etc.) skip them.
```bash
# Default run: includes dynamic scenarios
./examples/test/scripts/integration/run_integration.sh

# List / run a single scenario
./target/debug/test_client --list-dynamic-scenarios
./target/debug/test_client -g --dynamic-scenario <id>
```
For the scenario framework and how to add new scenarios see [the "Dynamic (hot reload) testing" section in 01-integration-testing.md](01-integration-testing.md).

### View the test report
```bash
cat ${WORK_DIR}/report.log
```

### Key test directories
- `edgion-tests/src/` — test code (client/server/validator)
- `examples/test/conf/` — test configurations (organized by resource type)
- `examples/test/scripts/` — test scripts
- `integration_testing/` — runtime working directory (gitignored)

### Read on demand
- [references/integration-suite-map.md](references/integration-suite-map.md) — suite family, conf directory, Rust suite, `--gateway` mapping
- [references/test-server-capabilities.md](references/test-server-capabilities.md) — `test_server` ports, HTTP endpoints, OIDC/auth, mirror, TLS backend capabilities

### Debugging and troubleshooting (moved to a separate directory)
- [../06-tracing/00-debugging.md](../06-tracing/00-debugging.md) — daily development debugging, Admin API, edgion-cli, quick lookup
