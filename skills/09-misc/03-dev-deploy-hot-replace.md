---
name: dev-deploy-hot-replace
description: End-to-end workflow for deploying Edgion to K8s using stock images with shell restart loop, cross-compiling locally, and hot-replacing binaries via kubectl cp.
---

# Dev Deploy & Hot-Replace Workflow

Use stock Docker images with a shell `command` restart loop, cross-compile
locally, then `kubectl cp` + `pkill` to hot-replace binaries without rebuilding
images.

## Overview

```
┌─ Local Machine ─────────────────────────┐
│  cargo zigbuild (controller, gateway)   │
│  go test -c    (conformance runner)     │
└───────────────┬─────────────────────────┘
                │ kubectl cp
                ▼
┌─ K8s Cluster (edgion-system) ───────────┐
│  Pod: edgion-controller                 │
│    PID 1: shell loop (while true; do…)  │
│    → /usr/local/edgion/edgion-controller│
│                                         │
│  Pod: edgion-gateway                    │
│    PID 1: shell loop (while true; do…)  │
│    → /usr/local/edgion/edgion-gateway   │
│                                         │
│  Pod: conformance-runner                │
│    → /opt/conformance/conformance-test  │
└─────────────────────────────────────────┘
```

## 1. Image & Version

Stock images from Docker Hub. The Deployment manifests use `imagePullPolicy:
IfNotPresent` so the image is pulled once and reused.

| Component  | Image                                      | Tag example        |
|------------|--------------------------------------------|--------------------|
| Controller | `docker.io/pandaala/edgion-controller`     | `v0.1.1`           |
| Gateway    | `docker.io/pandaala/edgion-gateway`        | `v0.1.1`           |
| Runner     | Same controller image (reused as base)     | `conformance-dev`  |

For conformance testing the `setup-orbstack.sh` script builds local images
tagged `conformance-dev` and sets `imagePullPolicy: Never`.

## 2. Deployment Command — Shell Restart Loop

Both controller and gateway Deployments override the default entrypoint with a
`while true` shell loop. This is the key enabler for hot-replace:

```yaml
# examples/k8stest/kubernetes/controller/deployment.yaml (excerpt)
command:
- "/bin/sh"
- "-c"
- |
  while true; do
    if [ -f /usr/local/edgion/edgion-controller ]; then
      BINARY=/usr/local/edgion/edgion-controller
    else
      BINARY=/usr/local/bin/edgion-controller
    fi
    "$BINARY" --config-file /usr/local/edgion/config/edgion-controller.yaml --test-mode || true
    echo "[restart] edgion-controller exited, restarting in 2s..."
    sleep 2
  done
```

**Design:**

- PID 1 is always the `/bin/sh` loop, NOT the business binary.
- Binary lookup order: `/usr/local/edgion/` (writable overlay) →
  `/usr/local/bin/` (image default, root-owned, read-only).
- After `pkill`, the loop automatically restarts with the new binary.
- `|| true` prevents CrashLoopBackOff when the binary exits non-zero.

Gateway uses the same pattern with `--integration-testing-mode`.

**IMPORTANT:** Never `kill 1`. Killing PID 1 causes the container runtime to
restart the container from the original image — any `kubectl cp`'d binary is
lost.

## 3. Cross-Compile Locally

### Rust binaries (controller & gateway)

```bash
# Requires: cargo-zigbuild, zig
cargo zigbuild --target x86_64-unknown-linux-gnu --bin edgion-controller --release
cargo zigbuild --target x86_64-unknown-linux-gnu --bin edgion-gateway --release

# Output:
#   target/x86_64-unknown-linux-gnu/release/edgion-controller
#   target/x86_64-unknown-linux-gnu/release/edgion-gateway
```

For ARM64 clusters replace with `aarch64-unknown-linux-gnu`.

### Conformance runner (Go)

```bash
cd examples/gateway-api-conformance
GOOS=linux GOARCH=amd64 CGO_ENABLED=0 go test -c -o conformance-test.bin .
```

## 4. Upload & Hot-Replace

### Controller & Gateway

```bash
NS="edgion-system"

# Get pod names
CTRL_POD=$(kubectl get pods -n $NS -l app=edgion-controller -o jsonpath='{.items[0].metadata.name}')
GW_POD=$(kubectl get pods -n $NS -l app=edgion-gateway -o jsonpath='{.items[0].metadata.name}')

# Upload to /tmp first (always writable)
kubectl cp target/x86_64-unknown-linux-gnu/release/edgion-controller $NS/$CTRL_POD:/tmp/edgion-controller
kubectl cp target/x86_64-unknown-linux-gnu/release/edgion-gateway    $NS/$GW_POD:/tmp/edgion-gateway

# Move to runtime overlay path (/usr/local/edgion/ is owned by edgion user)
kubectl exec -n $NS $CTRL_POD -- sh -c \
  "mv /tmp/edgion-controller /usr/local/edgion/edgion-controller && chmod +x /usr/local/edgion/edgion-controller"
kubectl exec -n $NS $GW_POD -- sh -c \
  "mv /tmp/edgion-gateway /usr/local/edgion/edgion-gateway && chmod +x /usr/local/edgion/edgion-gateway"

# Restart: kill the business process, NOT PID 1
kubectl exec -n $NS $CTRL_POD -- pkill -f edgion-controller
kubectl exec -n $NS $GW_POD   -- pkill -f edgion-gateway
```

### Conformance Runner

```bash
kubectl cp --retries=5 examples/gateway-api-conformance/conformance-test.bin \
  $NS/conformance-runner:/opt/conformance/conformance-test.bin
```

### Verify

```bash
# Wait a few seconds for restart loop
sleep 8

kubectl get pods -n $NS
kubectl get gatewayclass edgion -o jsonpath='{.status.conditions[?(@.type=="Accepted")].status}'
# Should print: True
```

## 5. Path Permissions Summary

| Path                     | Owner  | Writable by edgion user | Purpose           |
|--------------------------|--------|------------------------|--------------------|
| `/usr/local/bin/`        | root   | No                     | Image default binary |
| `/usr/local/edgion/`     | edgion | Yes                    | Runtime overlay    |
| `/tmp/`                  | any    | Yes                    | Staging area       |

Cannot `kubectl cp` directly to `/usr/local/bin/` (Permission denied) or
overwrite a running binary there (Text file busy). Always stage in `/tmp/` then
`mv` to `/usr/local/edgion/`.

## 6. Common Pitfalls

| Problem | Cause | Fix |
|---------|-------|-----|
| `tar: Cannot open: File exists` | `kubectl cp` to a path with running binary | Upload to `/tmp/` first, then `mv` |
| `Permission denied` on `mv` | Target dir owned by root | Use `/usr/local/edgion/` not `/usr/local/bin/` |
| Binary lost after restart | Killed PID 1 | Kill the child process, not PID 1 |
| `Text file busy` | Overwriting running binary in-place | `rm -f` then `cp`, or use `mv` (atomic) |
| Wrong architecture | Built for darwin/arm64 | Use `cargo zigbuild --target x86_64-unknown-linux-gnu` |
| GatewayClass not Accepted | Controller crashed on new binary | Check logs: `kubectl logs -n $NS $CTRL_POD --tail=50` |

## 7. Related Docs

- Deployment manifests: `examples/k8stest/kubernetes/{controller,gateway}/deployment.yaml`
- Local build details: [cicd/00-local-build.md](cicd/00-local-build.md)
- Hot-replace steps (also in): [../06-tracing/00-debugging.md](../06-tracing/00-debugging.md) §7
- Conformance in-cluster runner: [../08-kubernetes/01-gateway-api/02-conformance-testing.md](../08-kubernetes/01-gateway-api/02-conformance-testing.md)
- setup-orbstack.sh (full env setup): `examples/gateway-api-conformance/scripts/setup-orbstack.sh`
