---
name: conformance-debug-cookbook
description: Conformance test troubleshooting recipes — pre-flight checks, common failure patterns, and fixes derived from real debugging sessions.
---

# Conformance Debug Cookbook

Real-world failure patterns and recipes accumulated from conformance debugging
sessions. Use this when a conformance run fails or behaves unexpectedly.

## 1. Pre-flight Checks

Run these **before** starting any conformance suite. Most "stuck" runs trace
back to a skipped pre-flight item.

### 1.1 Controller healthy

```bash
POD=$(kubectl get pods -n edgion-system -l app=edgion-controller \
  -o jsonpath='{.items[0].metadata.name}')

# Must see "listening on ..." or "started" in recent logs, zero ERROR lines
kubectl logs --tail=20 -n edgion-system "$POD" | grep -iE 'listen|started|error'
```

Healthy indicator: log lines showing gRPC server listening and reconcile loop
started, with **no** `ERROR` or `panic` lines in the last 20 lines.

### 1.2 Gateway healthy

```bash
POD=$(kubectl get pods -n edgion-system -l app=edgion-gateway \
  -o jsonpath='{.items[0].metadata.name}')

# Must see port bind lines (80, 443, 8443 …), zero ERROR lines
kubectl logs --tail=30 -n edgion-system "$POD" | grep -iE 'bindto|bindaddr|listen|error'
```

Healthy indicator: one `bindto`/`listen` line per expected port. If a port is
missing, the corresponding tests will time out with `connection refused`.

### 1.3 GatewayClass accepted

```bash
kubectl get gatewayclass edgion \
  -o jsonpath='{.status.conditions[?(@.type=="Accepted")].status}'
# Must print "True"
```

### 1.4 Manifests applied

Both manifest files must be applied **before** the gateway pod starts (Edgion
binds ports at startup only):

```bash
kubectl apply -f examples/gateway-api-conformance/manifests/conformance-gateway.yaml
kubectl apply -f examples/gateway-api-conformance/manifests/conformance-gateway-tls.yaml
```

Verify key Gateways exist:

```bash
kubectl get gateways -n gateway-conformance-infra
# Expected: same-namespace, same-namespace-with-https-listener, all-namespaces,
#           gateway-tlsroute, pre-bind-tls-terminate-8443, pre-bind-tls-mixed-8883, ...
```

### 1.5 No stale processes

```bash
# On the conformance runner pod (if using in-cluster runner)
kubectl exec -n edgion-system conformance-runner -- \
  ps aux 2>/dev/null | grep conformance-tes

# Locally
pgrep -f "gateway-api-conformance.test" && echo "STALE — kill first"
```

Kill any leftover `<defunct>` or lingering test processes before starting a new run.

---

## 2. Common Failure Patterns

### 2.1 Namespace / Gateway deleted by test cleanup

**Symptom:** Tests pass individually but fail in batch. Second half of tests see
`connection refused` or `not found` for Gateways.

**Root cause:** The conformance suite creates and deletes the
`gateway-conformance-infra` namespace (and its Gateways) between test groups
when `cleanup-base-resources=true`. Even with `false`, some tests delete
dynamically created Gateways.

**Fix:**
- Always use `--cleanup-base-resources=false` (this is the default in our config).
- If namespace disappears mid-run, re-apply manifests and **restart the gateway
  pod** so ports are re-bound:

```bash
kubectl apply -f examples/gateway-api-conformance/manifests/conformance-gateway.yaml
kubectl apply -f examples/gateway-api-conformance/manifests/conformance-gateway-tls.yaml

kubectl rollout restart deployment/edgion-gateway -n edgion-system
kubectl rollout status deployment/edgion-gateway -n edgion-system --timeout=60s
```

### 2.2 HTTP tests timeout with `connection refused` on port 80

**Symptom:** All `HTTPRoute*` and `GRPCRoute*` tests stuck at 60s per sub-test,
spamming `dial tcp <ip>:80: connect: connection refused`.

**Root cause:** The gateway pod was started with only the TLS manifest
(`conformance-gateway-tls.yaml`), which does not define a port-80 listener. No
HTTP Gateway → no port-80 bind → every HTTP sub-test burns through its full
`MaxTimeToConsistency` timeout.

**Fix:** Apply **both** manifest files before starting the gateway:

```bash
kubectl apply -f examples/gateway-api-conformance/manifests/conformance-gateway.yaml
kubectl apply -f examples/gateway-api-conformance/manifests/conformance-gateway-tls.yaml
```

Then restart the gateway pod to pick up the new ports.

**Impact:** With ~100+ HTTP sub-tests at 60s each, a missing port-80 turns a
10-minute run into a 2+ hour hang. Reducing `MaxTimeToConsistency` to 15s
mitigates this but does not fix the root cause.

### 2.3 TLS Terminate tests fail — 8443 TLS callback not triggered

**Symptom:** `TLSRouteTerminateSimpleSameNamespace` times out. Client gets EOF
during TLS handshake on port 8443.

**Diagnosis checklist:**
1. Port 8443 listening? `kubectl exec <gw-pod> -- ss -tlnp | grep 8443`
2. Controller pushed TLS cert? Check controller logs for the Secret name
   referenced by the 8443 listener.
3. Pingora TLS callback invoked? Gateway debug logs should show `tls_callback`
   for port 8443.

**Common causes:**
- `conformance-gateway-tls.yaml` not applied (8443 not pre-bound).
- The referenced Secret (`tls-terminate-checks-certificate`) does not exist in
  the `gateway-conformance-infra` namespace — the conformance suite creates it
  dynamically; if the namespace was cleaned up, the Secret is gone.
- Gateway restarted after manifest apply but before the Secret was re-created.

### 2.4 Stale conformance-test processes on runner pod

**Symptom:** New test run hangs immediately or produces no output. `ps aux`
shows multiple `conformance-test.bin` or `<defunct>` processes.

**Fix:**

```bash
kubectl exec -n edgion-system conformance-runner -- \
  sh -c 'for p in $(pgrep -f conformance-test); do kill -9 $p; done'
```

### 2.5 `kill PID 1` restarts container with old binary

**Symptom:** After `kubectl cp` of new binary, `kill 1` restarts the container
from the original image — the new binary is lost.

**Root cause:** PID 1 is a shell wrapper (`while true; do ./edgion-xxx; done`).
Killing PID 1 causes the container to restart from the image, discarding the
overlaid binary.

**Fix:** Kill the **application** subprocess, not PID 1:

```bash
kubectl exec -n edgion-system "$POD" -- \
  sh -c 'kill -9 $(pgrep -f edgion-controller | grep -v "^1$" | head -1)'
```

Or use `orbstack-dev.sh hotpatch c` which handles this correctly.

### 2.6 Controller pods crash with `Address already in use`

**Symptom:** Controller pod restarts repeatedly, logs show
`Error: Address already in use (os error 98)`.

**Root cause:** Multiple controller replicas competing for the same gRPC listen
port.

**Fix:**

```bash
kubectl scale deployment/edgion-controller -n edgion-system --replicas=1
```

### 2.7 Gateway shows `No Gateway resources available` at startup

**Symptom:** Gateway pod starts but immediately exits because no Gateway
resources exist in the cluster.

**Root cause:** Conformance test cleanup deleted the namespace and Gateways.

**Fix:** Re-apply manifests (see 2.1), then restart the gateway pod.

### 2.8 gRPC tests crash/hang via port-forward proxy

**Symptom:** `GRPCRouteWeight` test spams `DeadlineExceeded`, eventually the
`go test` process silently dies.

**Root cause:** gRPC (HTTP/2 long-lived streams) does not work reliably through
`kubectl port-forward`. The TAL gRPC proxy client encounters persistent
connection establishment failures.

**Fix:** Use in-cluster mode (see `02-conformance-testing.md`). If you must use
port-forward, do **not** set `GATEWAY_PROXY_ENABLE_GRPC=true`.

### 2.9 `kubectl exec` disconnects mid-test

**Symptom:** `kubectl exec` returns but the test is still running inside the pod.
Log file on the local machine is truncated.

**Root cause:** `kubectl exec` has idle/connection timeouts. Long-running output
piped through `kubectl exec > local-file` can lose data when the connection drops.

**Fix:** Use `nohup` inside the pod to write logs to a pod-local file:

```bash
kubectl exec -n edgion-system conformance-runner -- sh -c '
nohup /tmp/conformance-test -test.run=TestConformance -test.v ... \
  > /tmp/conformance.log 2>&1 &
echo "PID=$!"'
```

Then monitor and retrieve:

```bash
kubectl exec ... -- sh -c 'wc -l /tmp/conformance.log; tail -5 /tmp/conformance.log'
kubectl cp edgion-system/conformance-runner:/tmp/conformance.log /tmp/local.log
```

### 2.10 Docker Hub rate limit on runner pod image pull

**Symptom:** Runner pod stuck in `ErrImagePull` / `ImagePullBackOff`.

**Fix:** Use an image already cached in the cluster:

```bash
image: pandaala/edgion-controller:conformance-dev
imagePullPolicy: IfNotPresent
```

---

## 3. Run Efficiency Tips

### 3.1 In-cluster mode (recommended)

Avoids all OrbStack routing / port-forward / gRPC proxy issues. Full steps in
`02-conformance-testing.md` "In-Cluster 运行方式" section.

### 3.2 Profile-scoped runs

Run profiles separately to isolate failures:

```bash
GO_RUN_FILTER="TestConformance/HTTPRoute" bash scripts/run.sh
GO_RUN_FILTER="TestConformance/Gateway" bash scripts/run.sh
GO_RUN_FILTER="TestConformance/TLSRoute" bash scripts/run.sh
GO_RUN_FILTER="TestConformance/GRPC" bash scripts/run.sh
```

### 3.3 Quick health after restart

```bash
kubectl wait --for=condition=Accepted gatewayclass/edgion --timeout=30s
kubectl wait --for=condition=Ready pod -l app=edgion-gateway -n edgion-system --timeout=60s
kubectl get gateways -n gateway-conformance-infra \
  -o jsonpath='{range .items[*]}{.metadata.name}: {.status.addresses[0].value}{"\n"}{end}'
```

---

## 4. Manifest Architecture

| File | Ports | Purpose |
|------|-------|---------|
| `conformance-gateway.yaml` | 80, 443 | HTTP/HTTPS — required for HTTPRoute and GRPCRoute |
| `conformance-gateway-tls.yaml` | 443, 8443, 8883 | TLS Passthrough/Terminate — required for TLSRoute |

**Both files must be applied** for a full run. Gateway pod must be restarted
after applying new manifests that add ports (Edgion binds ports at startup only).
