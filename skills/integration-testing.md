# Edgion Integration Testing Guide

> How to add, run, and debug integration tests. All test code lives under `examples/test/`.

## Architecture Overview

```
Controller (Admin :5800)          Gateway (Admin :5900, Listeners :31xxx)
  ├─ Receives YAML via edgion-ctl   ├─ Syncs resources from Controller via gRPC
  ├─ Schema validation (CRD)        ├─ Preparse + builds runtime config
  └─ FileSystemWriter saves config  └─ Serves traffic on listener ports

test_server (:30001-30023, :30040)   test_client (Rust binary)
  ├─ HTTP/gRPC/WebSocket/TCP/UDP     ├─ Sends requests to Gateway listeners
  │   echo backends                  ├─ Validates response status/headers/body
  └─ Auth service (:30040)           └─ Reports PASS/FAIL per test case
```

## Test Execution Flow

`run_integration.sh` orchestrates the full flow:

```
Step 1: Build      →  prepare.sh (cargo build all binaries)
Step 2: Start      →  start_all_with_conf.sh
  ├─ 2a: Kill old processes, check ports
  ├─ 2b: Check binaries exist
  ├─ 2c: Create work dir (integration_testing/testing_YYYYMMDD_HHMMSS/)
  ├─ 2d: Copy CRD schemas to work dir
  ├─ 2e: Generate TLS certs (scripts/certs/*.sh)
  ├─ 2f: Start test_server → wait health
  ├─ 2g: Start controller  → wait health + ConfigServer ready
  ├─ 2h: Load base config (conf/base/*.yaml) via edgion-ctl
  ├─ 2i: Load test suite configs (conf/<Resource>/<Item>/) via edgion-ctl
  ├─ 2j: Start gateway → wait ready + LB preload
  └─ 2k: Verify resource sync (resource_diff)
Step 3: Run tests  →  test_client -g -r <Resource> -i <Item>
Step 4: Cleanup    →  kill_all.sh
```

### Running Tests

```bash
# Full test (build + start + all tests + cleanup)
./examples/test/scripts/integration/run_integration.sh

# Run specific resource
./examples/test/scripts/integration/run_integration.sh -r EdgionPlugins

# Run specific item
./examples/test/scripts/integration/run_integration.sh -r EdgionPlugins -i KeyAuth

# Skip build (iterating on test code after initial build)
./examples/test/scripts/integration/run_integration.sh --no-prepare -r EdgionPlugins -i KeyAuth

# Keep services alive after test (for manual debugging)
./examples/test/scripts/integration/run_integration.sh --keep-alive -r EdgionPlugins -i KeyAuth

# Include slow tests (e.g. timeout tests)
./examples/test/scripts/integration/run_integration.sh --full-test
```

## Directory Structure

```
examples/test/
├── conf/                                  # Test configuration (YAML resources)
│   ├── base/                              # Base config loaded for every test
│   │   ├── EdgionGatewayConfig.yaml
│   │   ├── GatewayClass.yaml
│   │   └── ReferenceGrant_*.yaml
│   ├── ports.json                         # Port registry (CRITICAL: no duplicates!)
│   ├── EdgionPlugins/                     # Plugin test configs
│   │   ├── base/Gateway.yaml              # Shared Gateway for all plugin tests
│   │   ├── KeyAuth/                       # Per-plugin test configs
│   │   │   ├── 01_Secret_*.yaml           # Numbered prefix for load order
│   │   │   ├── EdgionPlugins_*.yaml       # Plugin definition
│   │   │   └── HTTPRoute_*.yaml           # Route binding
│   │   └── <YourPlugin>/
│   ├── HTTPRoute/
│   │   ├── Basic/
│   │   ├── Match/
│   │   ├── Backend/
│   │   │   ├── LBRoundRobin/
│   │   │   ├── Timeout/
│   │   │   └── ...
│   │   ├── Filters/
│   │   └── Protocol/
│   ├── Gateway/
│   ├── GRPCRoute/
│   ├── TCPRoute/
│   ├── UDPRoute/
│   └── EdgionTls/
├── code/
│   ├── client/
│   │   ├── test_client.rs                 # Entry point (CLI parsing, suite dispatch)
│   │   ├── framework.rs                   # TestSuite trait, TestContext, TestRunner
│   │   ├── port_config.rs                 # Load ports.json
│   │   └── suites/                        # Test suite implementations
│   │       ├── mod.rs                     # Re-exports all suites
│   │       ├── edgion_plugins/
│   │       │   ├── mod.rs                 # Re-exports plugin suites
│   │       │   ├── key_auth/
│   │       │   │   ├── mod.rs             # pub use key_auth::KeyAuthTestSuite;
│   │       │   │   └── key_auth.rs        # impl TestSuite for KeyAuthTestSuite
│   │       │   └── <your_plugin>/
│   │       ├── http_route/
│   │       ├── gateway/
│   │       └── ...
│   ├── server/test_server.rs              # Echo backend
│   └── validator/
│       ├── resource_diff.rs               # Controller-Gateway sync checker
│       └── config_load_validator.rs
├── scripts/
│   ├── integration/run_integration.sh     # Main entry point
│   ├── certs/                             # TLS cert generators (not committed to repo)
│   │   ├── generate_tls_certs.sh
│   │   ├── generate_backend_certs.sh
│   │   └── generate_mtls_certs.sh
│   └── utils/
│       ├── prepare.sh                     # Build all binaries
│       ├── start_all_with_conf.sh         # Start services + load config
│       ├── load_conf.sh                   # Load individual suite config
│       └── kill_all.sh                    # Stop all services
└── certs/                                 # Generated cert output (gitignored)

config/crd/                                # CRD schemas (check before writing test YAML!)
├── edgion-crd/
│   ├── edgion_plugins_crd.yaml
│   └── ...
└── gateway-api/
```

## Adding a New Plugin Integration Test

### Checklist (7 steps)

1. **Check/update CRD** — if your plugin adds new config fields, update `config/crd/edgion-crd/edgion_plugins_crd.yaml`
2. **Allocate port** (if needed) — update `conf/ports.json`
3. **Write test config YAML** — `conf/EdgionPlugins/<YourPlugin>/`
4. **Write test suite Rust code** — `code/client/suites/edgion_plugins/<your_plugin>/`
5. **Register suite** — wire up in `mod.rs` files + `test_client.rs`
6. **Register in run_integration.sh** — add to EdgionPlugins case
7. **Test** — run and verify

### Step 1: Check/Update CRD

Before writing YAML, verify your new config fields exist in the CRD schema:

```bash
# Check if your plugin type is in the CRD
grep -i "yourPlugin" config/crd/edgion-crd/edgion_plugins_crd.yaml
```

If your plugin adds new config types or fields, update the CRD first. The controller validates YAML against CRD schemas on load.

### Step 2: Allocate Port (if needed)

Most EdgionPlugins share port `31180` (the `EdgionPlugins` suite port). You only need a new port if:
- Your plugin needs its own Gateway listener (e.g., TLS, special protocol)
- There's a port conflict with existing tests

If you need a new port, edit `conf/ports.json`:

```json
{
  "current_max": 31276,
  "suites": {
    "EdgionPlugins": {
      "http": 31180
    },
    "YourNewSuite": {
      "http": 31276
    }
  }
}
```

Rules:
- Increment `current_max` to your new port value
- Port range: 31000-32767
- Never reuse an existing port
- Keep `current_max` accurate

### Step 3: Write Test Config YAML

Create `examples/test/conf/EdgionPlugins/<YourPlugin>/`:

**File naming convention:**
- Use numbered prefixes for load order: `01_`, `02_`, ...
- Format: `Kind_namespace_name.yaml` (e.g., `EdgionPlugins_default_your-plugin.yaml`)
- Dependencies (Secret, Service) load first (lower number)

**Minimum files for a plugin test:**

1. **Plugin definition** — `EdgionPlugins_default_your-plugin.yaml`:

```yaml
apiVersion: edgion.io/v1alpha1
kind: EdgionPlugins
metadata:
  name: your-plugin
  namespace: default
spec:
  requestFilters:
    - type: yourPlugin
      config:
        someField: "value"
```

2. **HTTPRoute binding** — `HTTPRoute_default_your-plugin-test.yaml`:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: your-plugin-test
  namespace: default
spec:
  parentRefs:
    - name: edgion-plugins-gateway    # Shared Gateway from EdgionPlugins/base/
      namespace: edgion-test
      sectionName: http               # Uses port 31180
  hostnames:
    - "your-plugin-test.example.com"  # Unique hostname for routing
  rules:
    - backendRefs:
        - name: test-http             # Points to test_server
          port: 30001
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: your-plugin
```

3. **Secret** (if needed) — `01_Secret_default_your-secret.yaml`

**Important:** Use a unique `hostname` per test to avoid routing conflicts. The shared EdgionPlugins Gateway listens on port 31180 and routes by hostname.

### Step 4: Write Test Suite Code

Create `examples/test/code/client/suites/edgion_plugins/your_plugin/`:

**`mod.rs`:**

```rust
mod your_plugin;
pub use your_plugin::YourPluginTestSuite;
```

**`your_plugin.rs`:**

```rust
// YourPlugin Integration Test Suite
//
// Required config files (in examples/test/conf/EdgionPlugins/YourPlugin/):
// - EdgionPlugins_default_your-plugin.yaml
// - HTTPRoute_default_your-plugin-test.yaml

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct YourPluginTestSuite;

const TEST_HOST: &str = "your-plugin-test.example.com";

impl YourPluginTestSuite {
    fn test_basic_success() -> TestCase {
        TestCase::new(
            "basic_success",
            "Valid request returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());

                    match ctx.http_client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!("Request failed: {}", e),
                        ),
                    }
                })
            },
        )
    }

    fn test_unauthorized() -> TestCase {
        TestCase::new(
            "unauthorized_returns_401",
            "Missing auth returns 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());

                    match ctx.http_client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 401 {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 401, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!("Request failed: {}", e),
                        ),
                    }
                })
            },
        )
    }
}

impl TestSuite for YourPluginTestSuite {
    fn name(&self) -> &str {
        "YourPlugin"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_basic_success(),
            Self::test_unauthorized(),
        ]
    }
}
```

**Key patterns:**
- Each test is a static method returning `TestCase`
- Use `Box::pin(async move { ... })` for async test body
- Always use `ctx.http_url()` — port comes from `ports.json`
- Set `host` header to match YAML hostname for routing through Gateway
- Return `TestResult::passed()` / `TestResult::failed()` with timing

### Step 5: Register Suite

**5a.** `code/client/suites/edgion_plugins/mod.rs` — add:

```rust
mod your_plugin;
pub use your_plugin::YourPluginTestSuite;
```

**5b.** `code/client/suites/mod.rs` — add to re-exports:

```rust
pub use edgion_plugins::YourPluginTestSuite;
```

**5c.** `code/client/test_client.rs` — add in **three** places:

1. `suite_to_port_key()`:

```rust
"EdgionPlugins/YourPlugin" => "EdgionPlugins",
```

2. `add_suites_for_suite()`:

```rust
"EdgionPlugins/YourPlugin" => {
    if !gateway {
        eprintln!("Error: EdgionPlugins/YourPlugin tests require --gateway flag");
        std::process::exit(1);
    }
    runner.add_suite(Box::new(suites::YourPluginTestSuite));
}
```

3. (Optional) Add legacy command alias in `resolve_suite()` if desired.

### Step 6: Register in run_integration.sh

Add to the `EdgionPlugins)` case in `run_all_tests()`:

```bash
# In the "if [ -z "$G_ITEM" ]" block (run all EdgionPlugins):
run_test "EdgionPlugins_YourPlugin" "${PROJECT_ROOT}/target/debug/examples/test_client -g -r EdgionPlugins -i YourPlugin" || test_failed=true
```

Also add to the `--suites` auto-inference in `main()` (the long comma-separated list for EdgionPlugins):

```bash
suites="${base_suites},EdgionPlugins/DebugAccessLog,...,EdgionPlugins/YourPlugin"
```

### Step 7: Test

```bash
# Build everything
./examples/test/scripts/utils/prepare.sh

# Run only your new test
./examples/test/scripts/integration/run_integration.sh -r EdgionPlugins -i YourPlugin

# Or keep services alive for iteration
./examples/test/scripts/integration/run_integration.sh --keep-alive -r EdgionPlugins -i YourPlugin

# After --keep-alive, run test_client directly without restart:
./target/debug/examples/test_client -g -r EdgionPlugins -i YourPlugin
```

## Test Config Correspondence Map

Test configs and code suites follow a **1:1 correspondence**:

| `conf/` directory | `code/client/suites/` directory | `run_integration.sh` command |
|---|---|---|
| `EdgionPlugins/KeyAuth/` | `edgion_plugins/key_auth/` | `-r EdgionPlugins -i KeyAuth` |
| `HTTPRoute/Match/` | `http_route/match/` | `-r HTTPRoute -i Match` |
| `Gateway/Security/` | `gateway/security/` | `-r Gateway -i Security` |
| `EdgionTls/mTLS/` | `edgion_tls/mtls/` | `-r EdgionTls -i mTLS` |

Keep this mapping clean when adding new tests.

## TLS Certificates

Certs are **never committed** to the repository. They are generated on-the-fly:

| Script | Output | Used by |
|---|---|---|
| `scripts/certs/generate_tls_certs.sh` | `examples/test/certs/tls/` | HTTPS, GatewayTLS tests |
| `scripts/certs/generate_backend_certs.sh` | `examples/test/certs/backend/` | Backend TLS tests |
| `scripts/certs/generate_mtls_certs.sh` | `examples/test/certs/mtls/` | mTLS tests |

If your plugin needs TLS (e.g., calling external HTTPS endpoints), either:
- Reuse existing certs from `examples/test/certs/`
- Add a new generator script in `scripts/certs/` and call it from `start_all_with_conf.sh` → `generate_certs()`

## Debugging Failed Tests

Follow this order — most issues are caught in the first 3 steps.

### 1. Code ↔ CRD Match

Is your Rust config struct in sync with the CRD schema?

```bash
# Check CRD has your plugin type
grep -i "yourPlugin" config/crd/edgion-crd/edgion_plugins_crd.yaml

# Check Rust enum has the variant
grep -i "YourPlugin" src/types/resources/edgion_plugins/edgion_plugin.rs
```

Common issue: added a new field in Rust config but forgot to regenerate/update CRD. Controller will reject the YAML silently.

### 2. Test YAML ↔ CRD Match

Is your test YAML valid against the CRD?

```bash
# Try loading manually
./target/debug/edgion-ctl --server http://127.0.0.1:5800 apply -f examples/test/conf/EdgionPlugins/YourPlugin/
```

Look for validation errors in controller log. Common issues:
- Wrong field name (camelCase mismatch)
- Missing required field
- Wrong type (string vs integer)
- Invalid enum value

### 3. Test Triad: test_server ↔ test_client ↔ conf YAML

Check these align:

| Check | What to verify |
|---|---|
| **Hostname** | YAML `hostnames` == test code `TEST_HOST` constant |
| **Port** | YAML `parentRefs.sectionName` → Gateway listener → `ports.json` entry → `suite_to_port_key()` mapping |
| **Backend** | YAML `backendRefs.port` == test_server listening port (30001 for HTTP) |
| **Path** | test code request path must match test_server endpoint (e.g., `/health`, `/headers`) |

### 4. Resource Sync

Check if resources reached the Gateway:

```bash
# Preferred: check logs
cat integration_testing/testing_*/logs/controller.log | grep -i "error\|warn\|reject"
cat integration_testing/testing_*/logs/gateway.log | grep -i "error\|warn\|reject"

# Via resource_diff tool
./target/debug/examples/resource_diff \
  --controller-url http://127.0.0.1:5800 \
  --gateway-url http://127.0.0.1:5900

# Via admin API (edgion-ctl)
./target/debug/edgion-ctl --server http://127.0.0.1:5800 get EdgionPlugins -A
./target/debug/edgion-ctl --server http://127.0.0.1:5900 get EdgionPlugins -A
```

### 5. Controller/Gateway Preparse

Both controller and gateway run preparse on resources. Check for preparse errors:

```bash
# Controller preparse
grep -i "preparse\|validation\|invalid" integration_testing/testing_*/logs/controller.log

# Gateway preparse
grep -i "preparse\|validation\|invalid" integration_testing/testing_*/logs/gateway.log
```

### 6. Live Debug with Logs

When all config looks correct but tests still fail:

```bash
# Run with --keep-alive so services stay up
./examples/test/scripts/integration/run_integration.sh --keep-alive -r EdgionPlugins -i YourPlugin

# Tail logs in separate terminals
tail -f integration_testing/testing_*/logs/gateway.log
tail -f integration_testing/testing_*/logs/controller.log
tail -f integration_testing/testing_*/logs/access.log

# Run test_client manually (repeat as you fix issues)
./target/debug/examples/test_client -g -r EdgionPlugins -i YourPlugin
```

### Log Locations

All logs are in the timestamped work directory:

```
integration_testing/testing_YYYYMMDD_HHMMSS/
├── logs/
│   ├── controller.log        # Controller stderr (errors, warnings, info)
│   ├── gateway.log           # Gateway stderr
│   ├── access.log            # Gateway access log (plugin_log included!)
│   ├── test_server.log       # Backend echo server
│   └── load_config.log       # Config loading output
├── test_logs/
│   ├── EdgionPlugins_YourPlugin.log  # Per-test stdout/stderr
│   └── ...
├── report.log                # Summary: PASS/FAIL per test
└── info.txt                  # PIDs, ports, work dir info
```

The **access log** is especially valuable — it contains `plugin_log` entries showing what each plugin did for each request.

### Quick Diagnosis Cheat Sheet

| Symptom | Likely Cause | Check |
|---|---|---|
| 404 Not Found | Hostname mismatch or route not loaded | YAML hostname, test HOST constant, Gateway listeners |
| 502 Bad Gateway | Backend unreachable | test_server running? Backend port correct? |
| 503 Service Unavailable | No healthy upstream | EndpointSlice/Service config, resource sync |
| Config load rejected | CRD mismatch | Controller log, CRD schema |
| Test hangs | Wrong port, service not started | `ports.json`, process list, port check |
| Passes alone, fails in full run | Port or hostname conflict | `ports.json` duplicates, hostname uniqueness |
