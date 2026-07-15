# LinkSys Integration Testing Guide

> How to add, run, and debug integration tests for LinkSys connectors (Redis, Etcd, Elasticsearch, future Kafka, etc.).
>
> **Redis** is integrated into the main runner (`run_integration.sh`) — invoked as the `LinkSys/Redis` suite, with assertions in `link_sys/redis_assertions.sh`. The runner auto-starts the Redis docker container as a precondition.
>
> **Etcd / Elasticsearch / future types** still ship as standalone scripts (`run_<type>_test.sh`) that manage their own lifecycle. New LinkSys types should follow the standalone-script pattern first; once stable, consider merging into the main runner the same way Redis was (assertions extracted into a `<type>_assertions.sh` driver, suite auto-discovered, runner adds a precondition + run_test invocation).

## Architecture Overview

```
External Service (Docker)          Controller (Admin :12101)
  e.g. Redis :16379                  ├─ Receives LinkSys YAML via edgion-cli
                                     └─ Syncs to Gateway via gRPC
                                   
Gateway (Admin :12001)             Test Script (bash)
  ├─ ConfHandler creates             ├─ Starts external service (Docker)
  │   runtime client                 ├─ Starts Controller + Gateway
  ├─ Exposes testing API             ├─ Loads LinkSys config via edgion-cli
  │   /api/v1/testing/link-sys/      ├─ Calls Gateway Admin API to test
  └─ Serves traffic                  └─ Reports PASS/FAIL per test case
```

### How It Differs from Plugin Integration Tests

| Aspect | Plugin tests | LinkSys tests |
|--------|-------------|---------------|
| **External dependency** | None (test_server is built-in) | Docker container (Redis, Kafka, etc.) |
| **Test runner** | Rust `test_client` binary | Bash script with `curl` + `jq` |
| **Verification** | Traffic through Gateway listener → response/access-log | Admin API endpoints that exercise runtime client ops |
| **Port allocation** | `ports.json` (Gateway listener ports) | Docker port mapping (e.g. `16379:6379`) |
| **Config scope** | `EdgionPlugins` + `HTTPRoute` + `Gateway` | `LinkSys` CRD only |

## Test Execution Flow

```
run_<type>_test.sh orchestrates the full flow:

Step 0: Stop old    →  use the integration harness cleanup helper
Step 1: Build       →  cargo build --bin edgion-controller --bin edgion-gateway --bin edgion-cli
Step 2: Work dir    →  mkdir tmp/integration_testing/<type>_testing_YYYYMMDD_HHMMSS/
Step 3: Start Infra →  docker compose up -d (e.g. Redis)
Step 4: Start Edgion (same args as start_all_with_conf.sh)
  ├─ 4a: Start Controller (--work-dir, --conf-dir, --test-mode) → wait /ready
  ├─ 4b: Load base config (conf/base/*.yaml) via edgion-cli
  ├─ 4c: Load LinkSys config (conf/LinkSys/<Type>/*.yaml) via edgion-cli
  └─ 4d: Start Gateway (--work-dir, --integration-testing-mode) → wait /ready + LB preload
Step 5: Run tests   →  curl Gateway Admin API testing endpoints
Step 6: Report      →  summarise PASS/FAIL
Step 7: Cleanup     →  kill processes, docker compose down
```

### Running Tests

**Redis (merged into the main runner):**

```bash
# Targeted: run only the LinkSys/Redis suite (auto-starts Redis docker)
./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r LinkSys -i Redis

# Default fast sweep (also runs LinkSys/Redis with the lifecycle delete
# assertion; --full-test only adds slow suites like LdapAuth and HealthCheckTransition)
./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare

# Run the assertions directly against an already-running stack (build skipped,
# services up, LinkSys/Redis CRDs applied)
bash ./edgion-tests/integration/scripts/integration/link_sys/redis_assertions.sh
bash ./edgion-tests/integration/scripts/integration/link_sys/redis_assertions.sh --include-lifecycle
```

**Etcd / Elasticsearch (still standalone):**

```bash
./edgion-tests/integration/scripts/integration/link_sys/run_etcd_test.sh           # full lifecycle
./edgion-tests/integration/scripts/integration/link_sys/run_etcd_test.sh --no-build
./edgion-tests/integration/scripts/integration/link_sys/run_etcd_test.sh --keep-alive

./edgion-tests/integration/scripts/integration/link_sys/run_es_test.sh
```

## Directory Structure

```
edgion-tests/
├── conf/
│   ├── base/                              # Base config (GatewayClass, EdgionGatewayConfig)
│   ├── Services/
│   │   ├── redis/
│   │   │   └── docker-compose.yaml        # Redis container for testing
│   │   └── <future>/
│   │       └── docker-compose.yaml
│   └── LinkSys/
│       ├── Redis/
│       │   ├── 00_Secret_default_redis-test-auth.yaml
│       │   └── 01_LinkSys_default_redis-test.yaml
│       ├── Etcd/
│       │   └── 01_LinkSys_default_etcd-*.yaml
│       ├── Elasticsearch/
│       │   └── 01_LinkSys_default_es-test.yaml
│       └── <Future>/
│           └── *.yaml
├── scripts/
│   └── integration/
│       ├── run_integration.sh             # ← Main runner; auto-starts Redis,
│       │                                  #   runs LinkSys/Redis assertions
│       └── link_sys/
│           ├── redis_assertions.sh        # ← Redis driver (invoked by the
│           │                              #   main runner; can also be run
│           │                              #   directly against a live stack)
│           ├── run_etcd_test.sh           # ← Standalone (own lifecycle)
│           ├── run_es_test.sh             # ← Standalone (own lifecycle)
│           └── run_<future>_test.sh       # ← Future: Kafka, etc.
└── ...

edgion-gateway/src/api/mod.rs               # Gateway Admin API with testing endpoints
edgion-gateway/src/link_sys/providers/redis/ # Redis runtime client (tested by the script)
```

## Gateway Admin API: LinkSys Testing Endpoints

These endpoints are **only available when Gateway runs with `--integration-testing-mode`**. They live under `/api/v1/testing/link-sys/redis/`.

### Client Management

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1/testing/link-sys/redis/clients` | List all registered Redis client names |
| `GET` | `/api/v1/testing/link-sys/redis/health` | Health check ALL Redis clients |
| `GET` | `/api/v1/testing/link-sys/redis/{name}/health` | Health check a single client |
| `GET` | `/api/v1/testing/link-sys/redis/{name}/ping` | PING with latency measurement |

### Key-Value Operations

| Method | Path | Body | Description |
|--------|------|------|-------------|
| `POST` | `/redis/{name}/set` | `{"key":"k","value":"v","ttl_seconds":60}` | SET (optional TTL) |
| `GET` | `/redis/{name}/get/{key}` | — | GET |
| `POST` | `/redis/{name}/del` | `{"keys":["k1","k2"]}` | DEL (batch) |
| `POST` | `/redis/{name}/incr/{key}` | — | INCR |

### Hash Operations

| Method | Path | Body | Description |
|--------|------|------|-------------|
| `POST` | `/redis/{name}/hset` | `{"key":"h","field":"f","value":"v"}` | HSET |
| `GET` | `/redis/{name}/hget/{key}/{field}` | — | HGET |
| `GET` | `/redis/{name}/hgetall/{key}` | — | HGETALL |

### List Operations

| Method | Path | Body | Description |
|--------|------|------|-------------|
| `POST` | `/redis/{name}/rpush` | `{"key":"l","values":["a","b"]}` | RPUSH |
| `GET` | `/redis/{name}/lpop/{key}` | — | LPOP |
| `GET` | `/redis/{name}/llen/{key}` | — | LLEN |

### Distributed Lock

| Method | Path | Body | Description |
|--------|------|------|-------------|
| `POST` | `/redis/{name}/lock` | `{"key":"lk","ttl_seconds":5,"max_wait_seconds":3}` | Acquire + release lock |

> **`{name}` format:** `namespace_name` (underscore-separated). Example: `default_redis-test`

### Response Format

All endpoints return the standard `ApiResponse<T>`:

```json
{
  "success": true,
  "data": { ... },
  "error": null
}
```

On failure:

```json
{
  "success": false,
  "data": null,
  "error": "Redis client 'default/nonexistent' not found. Available: [\"default/redis-test\"]"
}
```

## Adding a New LinkSys Type Integration Test

### Checklist (6 steps)

1. **Create Docker Compose** — `conf/Services/<type>/docker-compose.yaml`
2. **Write LinkSys CRD YAML** — `conf/LinkSys/<Type>/01_LinkSys_*.yaml`
3. **Add Gateway Admin API testing endpoints** — `edgion-gateway/src/api/mod.rs`
4. **Write test script** — `scripts/integration/link_sys/run_<type>_test.sh`
5. **Build & test** — run the script
6. **Update this skill doc** — add the new type's info

### Step 1: Docker Compose for External Service

Create `edgion-tests/integration/conf/Services/<type>/docker-compose.yaml`:

```yaml
version: "3.8"
services:
  <type>:
    image: <official-image>:<tag>
    container_name: edgion-test-<type>
    ports:
      - "<test-port>:<service-port>"
    healthcheck:
      test: [...]
      interval: 2s
      timeout: 3s
      retries: 10
    restart: "no"
```

**Port allocation rules for Docker:**
- Use high ports (15000-19999) to avoid conflicts with host services
- Document the mapping in the compose file comment
- Current allocations:
  - Redis: `16379:6379`

### Step 2: LinkSys CRD YAML

Create `edgion-tests/integration/conf/LinkSys/<Type>/01_LinkSys_default_<name>.yaml`:

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: <type>-test
  namespace: default
spec:
  type: <type>
  config:
    # ... type-specific config matching RedisClientConfig / FutureConfig ...
```

**Naming convention:** `01_LinkSys_<namespace>_<name>.yaml`

### Step 3: Gateway Admin API Testing Endpoints

Add testing endpoints to `edgion-gateway/src/api/mod.rs` inside the `create_testing_router()` function. Follow the Redis pattern:

```rust
// In create_testing_router():
.route("/api/v1/testing/link-sys/<type>/health", get(<type>_health))
.route("/api/v1/testing/link-sys/<type>/{name}/ping", get(<type>_ping))
// ... type-specific operation endpoints ...
```

**Key design principles:**
- Every admin endpoint should be idempotent where possible
- Return standard `ApiResponse<T>` format
- Include the client name lookup helper (`get_<type>_client_by_name`)
- Error responses should include the list of available clients

### Step 4: Write Test Script

Create `edgion-tests/integration/scripts/integration/link_sys/run_<type>_test.sh`. Use `run_etcd_test.sh` or `run_es_test.sh` as a template (the previous Redis standalone has been merged into the main runner; if you ever want to merge a new type the same way, see `redis_assertions.sh` + the `is_redis_service_required` / `start_redis_service_if_needed` plumbing in `run_integration.sh` for the wiring pattern).

**Script structure:**

```bash
#!/bin/bash
# 1. Parse args (--no-cleanup, --no-build, --keep-alive)
# 2. Define helpers (log_*, assert_*, gw_api)
# 3. Cleanup trap
# 4. Build (optional)
# 5. Start external service (Docker)
# 6. Start Controller → load config → start Gateway
# 7. Run test cases via curl + assert_*
# 8. Report results
```

**Test assertion helpers (copy from `redis_assertions.sh` / `run_etcd_test.sh`):**

```bash
assert_eq()       # Exact string match
assert_contains() # Substring match
assert_success()  # JSON .success == true
assert_failure()  # JSON .success == false
```

**Test categories to cover for any LinkSys type:**

| Category | What to test |
|----------|-------------|
| **Resource sync** | LinkSys CRD visible in Gateway config cache |
| **Client registration** | Client appears in `/clients` list |
| **Health** | PING / health check returns healthy |
| **Core operations** | Type-specific read/write operations |
| **Error handling** | Non-existent client returns proper error |
| **Lifecycle** | Delete CRD → client removed; re-create → client restored |

### Step 5: Build & Test

```bash
# Full standalone run
./edgion-tests/integration/scripts/integration/link_sys/run_<type>_test.sh

# Iterating (skip build)
./edgion-tests/integration/scripts/integration/link_sys/run_<type>_test.sh --no-build

# Debug (keep alive)
./edgion-tests/integration/scripts/integration/link_sys/run_<type>_test.sh --keep-alive
```

**Optional: merge into the main runner** (Redis pattern). Once the standalone script is stable, you can fold its assertions into `run_integration.sh` so contributors get one entry point:

1. Extract the post-startup assertions into `link_sys/<type>_assertions.sh` (assumes services and CRDs are already up — no docker/cli plumbing in the driver).
2. Add a `Services/<type>/docker-compose.yaml` precondition to `run_integration.sh` (mirror `start_redis_service_if_needed` and `is_redis_service_required`).
3. Add `LinkSys/<Type>` to the `-r LinkSys` switch and the default sweep, with a small helper that formats the driver invocation and threads `--full-test` into the lifecycle flag if the type has destructive teardown assertions.
4. Update auto-loaded suites: when `-r LinkSys -i <Type>` is targeted, the runner should add `EdgionPlugins/base` (and any plugin suite the assertions exercise) so the right Gateway and routes exist.
5. Delete the original `run_<type>_test.sh` and update doc references.

### Step 6: Update This Document

Add the new type's Docker port, YAML example, and API endpoint table to this skill doc.

## Debugging Failed Tests

### 1. Docker Container Not Starting

```bash
# Check container status
docker ps -a | grep edgion-test

# Check container logs
cd edgion-tests/integration/conf/Services/<type> && docker compose logs

# Manual connectivity test
docker exec edgion-test-redis redis-cli -a edgion-test-pwd ping
```

### 2. LinkSys CRD Not Syncing

```bash
# Check if Controller accepted the CRD
curl -s http://127.0.0.1:12101/api/v1/namespaced/LinkSys/default | jq .

# Check if Gateway received it
curl -s "http://127.0.0.1:12001/configclient/LinkSys?namespace=default&name=redis-test" | jq .

# Check controller logs for errors
grep -i "error\|warn\|reject" <work-dir>/logs/controller.log
```

### 3. Redis Client Not Initialising

```bash
# Check Gateway logs for LinkSys handler output
grep -i "link_sys\|redis\|RedisLinkClient" <work-dir>/logs/gateway.log

# Check if client is registered
curl -s http://127.0.0.1:12001/api/v1/testing/link-sys/redis/clients | jq .

# Try a direct PING
curl -s http://127.0.0.1:12001/api/v1/testing/link-sys/redis/default_redis-test/ping | jq .
```

### 4. Config Mismatch

Common issues:
- Wrong Redis port in YAML (should be `16379` for Docker-mapped port)
- Wrong password (should match `docker-compose.yaml`)
- Topology mode mismatch (standalone Redis but config says `cluster`)

```bash
# Verify config matches
cat edgion-tests/integration/conf/LinkSys/Redis/01_LinkSys_default_redis-test.yaml
cat edgion-tests/integration/conf/Services/redis/docker-compose.yaml
```

### 5. Testing Endpoints Not Available

The testing endpoints are **only** available when Gateway runs with `--integration-testing-mode`. Check:

```bash
# This should return a valid response
curl -s http://127.0.0.1:12001/api/v1/testing/link-sys/redis/clients | jq .

# If it returns 404, Gateway is not in integration testing mode
grep "integration_testing" <work-dir>/logs/gateway.log
```

### Quick Diagnosis Cheat Sheet

| Symptom | Likely Cause | Check |
|---------|-------------|-------|
| Docker container won't start | Port conflict | `lsof -i :16379` |
| CRD 400 on apply | YAML schema mismatch | Controller log, CRD schema |
| Client not registered | Config mapping error | Gateway log: `link_sys` / `redis` |
| PING fails | Wrong endpoint/password | Verify Docker port + password match YAML |
| 404 on testing endpoints | Missing `--integration-testing-mode` | Gateway startup args |
| Lock test hangs | Redis not responding | `docker exec redis-cli ping` |
| Lifecycle test fails | Sync delay | Increase `sleep` between delete and check |

## Log Locations

All logs are in the timestamped work directory:

```
tmp/integration_testing/redis_testing_YYYYMMDD_HHMMSS/
├── logs/
│   ├── controller.log    # Controller stderr
│   └── gateway.log       # Gateway stderr (includes LinkSys handler logs)
└── report.log            # Summary: PASS/FAIL counts
```

## Current LinkSys Test Implementations

### Redis

| Item | Details |
|------|---------|
| **Runner integration** | Merged into `run_integration.sh` as the `LinkSys/Redis` suite (auto-discovered). Redis docker is auto-started via `start_redis_service_if_needed`. |
| **Assertions driver** | `edgion-tests/integration/scripts/integration/link_sys/redis_assertions.sh` (`--include-lifecycle` is always passed by the runner, so the lifecycle delete assertion runs in both fast and `--full-test` mode; the flag stays opt-in for direct invocations) |
| **Docker** | `edgion-tests/integration/conf/Services/redis/docker-compose.yaml` |
| **Config** | `edgion-tests/integration/conf/LinkSys/Redis/01_LinkSys_default_redis-test.yaml` |
| **Docker port** | `16379:6379` |
| **Password** | `edgion-test-pwd` (hardcoded for testing only) |
| **Client name** | `default/redis-test` (URL: `default_redis-test`) |
| **Test count** | 62 assertions across 10 sections (resource sync, client registration, health/PING, KV, hash, list, distributed lock, error handling, RateLimitRedis plugin, lifecycle delete) |
| **API base** | `/api/v1/testing/link-sys/redis/` |
