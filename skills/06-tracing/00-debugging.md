---
name: debugging-and-troubleshooting
description: Day-to-day development debugging guide for Edgion. Use when local code changes break routing, plugins, sync, readiness, or runtime behavior and you need a fast workflow using keep-alive integration runs, Admin API, edgion-cli, access-log store, metrics, and resource_diff.
---
# Debugging and troubleshooting

> Aimed at daily development troubleshooting — not "how to write tests" but "after this code change, which layer is now broken".
> If the issue is clearly concentrated in TLS Gateway, also read [../09-misc/debugging-tls-gateway.md](../09-misc/debugging-tls-gateway.md) first.

## Pick the debugging entry point first

### 1. Configuration / sync chain issues

Symptoms:
- `edgion-cli apply` succeeds but the Gateway doesn't react
- Controller `ready` never passes
- Gateway `ready` never passes
- YAML changed but `server` / `client` views are inconsistent

Use first:
- `edgion-cli --target center/server/client`
- `resource_diff`
- Controller/Gateway Admin API

### 2. Routing / plugin runtime issues

Symptoms:
- 404 / 421 / 502 / 503 returned
- Plugin not executed, in the wrong order, or condition not hit
- Route hit the wrong Gateway / Listener / Backend

Use first:
- Access Log Store
- Gateway Admin API
- Gateway logs and `store-stats`

### 3. Load-balancing / statistical-behavior issues

Symptoms:
- Uneven traffic distribution
- Unstable consistent hashing
- Retry count differs from expectation

Use first:
- `http://127.0.0.1:12090/metrics`
- `MetricsClient`
- `resource_diff`

## Fastest local debugging

### Preserve the scene and run a single case

```bash
./edgion-tests/integration/scripts/integration/run_integration.sh --keep-alive -r <Resource> -i <Item>
```

Good for:
- Bringing up the scene with the repo's existing test config
- Keeping controller / gateway / test_server processes and log directories after the run
- Then manually `curl`, `edgion-cli`, and read logs

### Skip compile for fast iteration

```bash
./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare --keep-alive -r <Resource> -i <Item>
```

### Manually start the full test environment

```bash
./edgion-tests/integration/scripts/utils/start_all_with_conf.sh
```

This will:
- Create `tmp/integration_testing/testing_YYYYMMDD_HHMMSS/`
- Export `EDGION_WORK_DIR`
- Start `test_server`
- Start the controller
- Load base config and suite config
- Start the gateway
- Finally run `resource_diff` once

## Step 1: confirm the services are actually alive

### Controller

```bash
# health/ready are on the dedicated probe port 12100; metrics on 12190; admin/business on 12101
curl -sf http://127.0.0.1:12100/health
curl -sf http://127.0.0.1:12100/ready
curl -sf http://127.0.0.1:12190/metrics
curl -sf http://127.0.0.1:12101/api/v1/server-info
```

### Gateway

```bash
curl -sf http://127.0.0.1:12000/health
curl -sf http://127.0.0.1:12000/ready
curl -sf http://127.0.0.1:12001/api/v1/server-info
curl -sf http://127.0.0.1:12090/metrics
```

Things to remember:
- The Gateway probe port (liveness/readiness) is `12000` (default `0.0.0.0:12000`), configurable via `gateway.probe_listen` in the config file or `--probe-listen` on the command line
- The Gateway Admin API default port is `12001` (loopback-only), configurable via `gateway.admin_listen` or `--admin-listen`; serves `/configclient`, `/api/v1/server-info`, and other internal endpoints
- The Gateway Metrics API port is `12090` (independent listener)

## Step 2: use the center / server / client three views to locate the issue layer

The three `edgion-cli` targets are great for fast localization:

```bash
./target/debug/edgion-cli get httproute -n default
./target/debug/edgion-cli -t server get httproute -n default
./target/debug/edgion-cli -t client get httproute -n default
```

Decision rules:

| Symptom | Probable issue layer |
|------|--------------|
| `center` has no resource | Config not really applied, or wrong kind / namespace / name |
| `center` has it, `server` doesn't | Controller `ProcessorHandler` validation / preparse / parse / requeue chain |
| `server` has it, `client` doesn't | gRPC sync, `no_sync_kinds`, Gateway readiness, watch/list flow |
| `client` has it, but traffic behavior is wrong | Gateway `ConfHandler`, runtime store, route/plugin/tls/backend logic |

For batch comparison, just run:

```bash
./target/debug/resource_diff
```

Or specify the addresses:

```bash
./target/debug/resource_diff \
  --controller-url http://127.0.0.1:12101 \
  --gateway-url http://127.0.0.1:12001
```

## Step 3: check key Admin APIs

### Controller side

- `/api/v1/server-info`
- `/api/v1/namespaced/{kind}`
- `/api/v1/cluster/{kind}`
- `/configserver/{kind}/list`
- `/configserver/{kind}?name=...&namespace=...`

Examples (all Controller-side endpoints require a Bearer token when authentication is enabled; if not logged in, run `edgion-cli login` first or read the token from `~/.edgion/token`):

```bash
TOKEN=$(cat ~/.edgion/token)
curl -s -H "Authorization: Bearer $TOKEN" http://127.0.0.1:12101/configserver/httproute/list | jq .
curl -s -H "Authorization: Bearer $TOKEN" "http://127.0.0.1:12101/configserver/httproute?namespace=default&name=my-route" | jq .
```

### Gateway side

- `/api/v1/server-info`
- `/configclient/{kind}/list`
- `/configclient/{kind}?name=...&namespace=...`
- `/api/v1/debug/store-stats`

Examples:

```bash
curl -s http://127.0.0.1:12001/configclient/httproute/list | jq .
curl -s http://127.0.0.1:12001/api/v1/debug/store-stats | jq .
```

`store-stats` is especially good for seeing:
- Whether objects are in the route manager
- Whether the TLS store / plugin store / backend policy store has leaks or residue

## Step 4: capture Access Log Store to see what plugins and routes actually executed

Prerequisite: the Gateway must start with `--integration-testing-mode`.

### Capture by direct HTTP request

1. Send the request with:
   - `x-trace-id: <unique value>`
   - `access_log: test_store`
2. Then query:

```bash
curl -s http://127.0.0.1:12001/api/v1/testing/status | jq .
curl -s http://127.0.0.1:12001/api/v1/testing/access-log/<trace-id> | jq .
```

### Focus on these fields

- `request_info.host`
- `request_info.path`
- `matched_route`
- `backend`
- `stage_logs`
- `diagnostics`

For plugin issues, `stage_logs` is the most useful; for upstream issues, look first at `backend.upstreams` and `diagnostics`.

A ready-made client on the code side:
- `edgion-tests/src/client/access_log_client.rs`

## Step 5: check Metrics

The metrics service is at:

```bash
curl -s http://127.0.0.1:12090/metrics
```

Good for verifying:
- Load-balancing distribution
- Consistent hashing
- Retry and upstream request counts
- Log-drop counters

For "statistical behavior is wrong" issues, prefer metrics rather than just looking at a single response.

## Step 6: check log directories and key files

In an integration test scene, the most commonly used are:

- `${EDGION_WORK_DIR}/logs/gateway.log`
- `${EDGION_WORK_DIR}/logs/controller.log`
- `${EDGION_WORK_DIR}/logs/edgion_access.log`
- `${EDGION_WORK_DIR}/logs/ssl.log`
- `${EDGION_WORK_DIR}/logs/tls_access.log`

For a scene generated by `run_integration.sh`, look first at:

- `tmp/integration_testing/<timestamp>/logs/`
- `tmp/integration_testing/<timestamp>/test_logs/`
- `tmp/integration_testing/<timestamp>/report.log`

## Step 7: remote K8s cluster scene (without rebuilding the image)

Suitable when:
- The issue only reproduces in the remote cluster
- You don't want to rebuild + rollout images every time
- You need to push a freshly compiled local `edgion-gateway` / `edgion-controller` into an existing Pod for quick verification

### 7.1 Two prerequisites

- **The binary target must match the node architecture**. For example, if the remote cluster is `linux/amd64`, use `x86_64-unknown-linux-gnu`; for other architectures, swap to the appropriate target.
- **PID 1 is often not the business process but a shell loop**. Do not kill PID 1, otherwise the container will restart with the old binary from the image and the hot replacement is lost.

### 7.2 Hot-replace the binary inside the Pod

Two common facts:
- `/usr/local/bin/` is usually the original binary owned by root in the image and is not suitable to overwrite
- `/usr/local/edgion/` is the runtime override location; if a binary with the same name lives here, the entrypoint loop runs it preferentially

```bash
NS="edgion-system"
POD="edgion-gateway-xxxxx"
BIN="edgion-gateway"   # or edgion-controller
LOCAL="target/x86_64-unknown-linux-gnu/release/$BIN"

# 1. Copy the binary into a temp dir in the Pod
kubectl cp "$LOCAL" "$NS/$POD:/tmp/$BIN" -c "$BIN"

# 2. Install it atomically at the runtime-override path
kubectl exec -n "$NS" "$POD" -c "$BIN" -- sh -c \
  "chmod +x /tmp/$BIN && mv /tmp/$BIN /usr/local/edgion/$BIN"

# 3. Kill the business child process so the shell loop relaunches with the new binary
PID=$(kubectl exec -n "$NS" "$POD" -c "$BIN" -- \
  pgrep -f "^(/usr/local/edgion/|/usr/local/bin/)?$BIN( |$)" | grep -v '^1$' | head -1)
test -n "$PID"
kubectl exec -n "$NS" "$POD" -c "$BIN" -- kill -TERM "$PID"
```

Common pitfalls:
- `Text file busy`: stage under `/tmp`, then replace the path with `mv` as shown
- `Permission denied`: you wrote to `/usr/local/bin/`; switch to `/usr/local/edgion/`
- Logs still show the old error: most likely an old child process wasn't killed, or the old binary ran once between `cp` and `kill`

### 7.3 Quickly confirm the new binary is in effect

```bash
kubectl exec -n "$NS" "$POD" -c "$BIN" -- /usr/local/edgion/$BIN --version
kubectl exec -n "$NS" "$POD" -c "$BIN" -- md5sum /usr/local/edgion/$BIN
```

To confirm a specific old error string has disappeared, you can also check locally first:

```bash
strings "$LOCAL" | rg "expected error text"
```

### 7.4 Check ports and logs inside the Pod

First check which ports are currently bound:

```bash
kubectl exec -n "$NS" "$POD" -c edgion-gateway -- \
  cat /proc/$(pgrep -f edgion-gateway | grep -v '^1$' | head -1)/net/tcp
```

In-cluster log paths are usually at:

```text
/usr/local/edgion/logs/
├── edgion-gateway.log
├── edgion_access.log
├── tls_access.log
└── ssl.log
```

```bash
kubectl exec -n "$NS" "$POD" -c edgion-gateway -- \
  tail -50 /usr/local/edgion/logs/edgion-gateway.log
kubectl exec -n "$NS" "$POD" -c edgion-gateway -- \
  tail -20 /usr/local/edgion/logs/tls_access.log
kubectl logs -n "$NS" "$POD" -c edgion-gateway --since=120s
```

Distinguish:
- `kubectl logs` mostly shows the stdout/stderr of the PID 1 shell loop — good for startup errors, `anyhow::bail!`, `[restart]`
- For structured tracing, TLS access, and certificate errors, look at `/usr/local/edgion/logs/*` first

### 7.5 If you are debugging Gateway API conformance

- For in-cluster runner usage see [../08-kubernetes/01-gateway-api/02-conformance-testing.md](../08-kubernetes/01-gateway-api/02-conformance-testing.md)
- For TLS conformance / pre-bound listeners / failure modes see [../08-kubernetes/01-gateway-api/06-conformance-debug-cookbook.md](../08-kubernetes/01-gateway-api/06-conformance-debug-cookbook.md)

## Common-symptom quick lookup

| Symptom | Look here first |
|------|---------|
| `edgion-cli apply` succeeds but Gateway doesn't change | Three-view comparison + `resource_diff` |
| Controller `ready` doesn't pass | `gatewayclass` / `gateway` / `processor` logs; if needed, `/api/v1/server-info` |
| Gateway `ready` doesn't pass | `client`-side cache, `supported_kinds`, `server_id`, watch/list logs |
| 404 / no match | `configclient` resources, `store-stats`, Access Log `matched_route` |
| 502 / 503 | `Service` / `EndpointSlice` / `Endpoint`, backend store, metrics, upstream logs |
| Plugin not executed | `stage_logs` of the Access Log Store |
| TLS handshake or certificate issue | `ssl.log`, `tls_access.log`, [../09-misc/debugging-tls-gateway.md](../09-misc/debugging-tls-gateway.md) |

## If it is a configuration-layer issue

Many "why my YAML / work_dir / conf_dir change didn't take effect" issues are not about debugging but about choosing the wrong configuration layer.
For those, jump to:

- [../02-features/02-config/SKILL.md](../02-features/02-config/SKILL.md)

## Code entry-point quick reference

- Controller Admin API: `edgion-controller/src/api/`
- Gateway Admin API: `edgion-gateway/src/api/mod.rs`
- `edgion-cli`: `edgion-cli/src/cli/`
- Gateway startup entry: `edgion-gateway/src/cli/mod.rs`
- Controller startup entry: `edgion-controller/src/cli/mod.rs`
- Resource sync verification tool: `edgion-tests/src/validator/resource_diff.rs`
