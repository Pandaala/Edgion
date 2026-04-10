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

### 2.3.1 Port 443 mixed-mode conflict (HTTPS Terminate + TLS Passthrough)

**Architectural constraint:** Edgion does **not** support mixing HTTPS Terminate
and TLS Passthrough on the same port. Conformance tests use port 443 for both
`same-namespace-with-https-listener` (HTTPS Terminate) and `gateway-tlsroute`
(TLS Passthrough). The upstream test code hardcodes the port from Gateway spec;
there is no way to remap them.

**Consequence:** GATEWAY-HTTP and GATEWAY-TLS profiles **must be run
separately** — never in the same invocation. When both manifests are present,
TLS Passthrough requests on 443 get intercepted by the HTTPS Terminate listener,
returning the wrong certificate or timing out.

**Affected tests when run together (all TLSRoute data-plane tests):**

- `TLSRouteSimpleSameNamespace` (core)
- `TLSRouteHostnameIntersection` (core)
- `TLSRouteTerminateSimpleSameNamespace` (extended)
- `TLSRouteInvalidBackendRefNonexistent` (core — data-plane sub-test)
- `TLSRouteInvalidBackendRefUnknownKind` (core — data-plane sub-test)

All TLSRoute status-only tests pass fine in mixed mode because they don't send
traffic through port 443.

**Run protocol:**

`in-cluster-runner.sh --clean-slate` handles everything automatically:

1. Deletes non-base resources from all conformance namespaces
2. Applies only the manifests needed for the active profiles
3. For TLS-only: injects `EDGION_BASE_MANIFESTS` pointing to
   `base-manifests-tls-only.yaml` so the framework does not recreate HTTP
   Gateways on port 443
4. Restarts controller + gateway processes
5. Saves reports to `reports/` with profile-derived filenames

```bash
# Step 1: GATEWAY-HTTP + GATEWAY-GRPC
./scripts/in-cluster-runner.sh --clean-slate \
  --conformance-profiles "GATEWAY-HTTP,GATEWAY-GRPC" --timeout 30m

# Step 2: GATEWAY-TLS only
./scripts/in-cluster-runner.sh --clean-slate \
  --conformance-profiles "GATEWAY-TLS" --timeout 30m
```

**Important:** the framework's `base/manifests.yaml` always applies base
Gateways (including `same-namespace-with-https-listener` on port 443). For TLS
runs, `base-manifests-tls-only.yaml` is a filtered copy of upstream base with
HTTP Gateways removed to prevent the port 443 conflict.

**Note on cert lifecycle:** `--clean-slate` also performs `rollout restart` on
all TLS backend deployments. This is necessary because cert Secrets are
regenerated by the framework with new key pairs, but running backend pods cache
the old certs in memory. See 2.14 for details.

### 2.3.2 Client-cert validation Gateways pollute port 443

**Symptom:** `HTTPRouteHTTPSListener` (GATEWAY-HTTP core) fails with
`tls: certificate required` on every HTTPS request to port 443.

**Root cause:** `client-validation-default` and
`client-validation-insecure-fallback` Gateways had HTTPS Terminate listeners on
port 443 with `tls.frontend.default.validation` (client cert required). These
Gateways served the `GatewayFrontendClientCertificateValidation` feature which
Edgion does **not** declare — no test exercises them, but their listeners
intercepted all port-443 HTTPS traffic and demanded client certificates.

**Fix:** These Gateways were removed from `conformance-gateway.yaml`. If
`GatewayFrontendClientCertificateValidation` is declared in the future, they
must use a dedicated port (not 443) to avoid conflicting with
`same-namespace-with-https-listener`.

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

### 2.8 Controller periodic restart breaks TLS data-plane tests (FIXED)

**Symptom:** TLSRoute data-plane tests fail with `certificate signed by unknown
authority` or `connection refused`. Controller logs show `Graceful shutdown timed
out 25s after signal, forcing exit` every ~27 seconds.

**Root cause:** `src/core/controller/cli/mod.rs` used
`tokio::time::timeout(25s, async { tokio::join!(...) })` which started the 25s
timer from service boot, not from shutdown signal. The controller exited after 25s
regardless of workload.

**Fix (applied 2026-04-07):** Replaced with `tokio::select!` — the 25s drain
deadline now starts only after `ShutdownHandle::signal().wait()` fires.

### 2.13 TLS cert x509 CA:FALSE — Go 1.24+ (not triggered in v1.5.1)

**Symptom:** All TLS data-plane tests fail with `tls: failed to verify certificate:
x509: certificate signed by unknown authority (possibly because of "x509: invalid
signature: parent certificate cannot sign this kind of certificate")`.

**Root cause:** The upstream conformance suite (`sigs.k8s.io/gateway-api/conformance
v1.5.1`) generates self-signed certificates with `BasicConstraints CA:FALSE`. Go
1.24+ rejects these when used in a root CA pool via `x509.Verify`.

**Status (verified 2026-04-09):** Conformance v1.5.1 TLS tests use
`InsecureSkipVerify` / do not call `x509.Verify` on client side, so this code
path is never triggered. All 21 GATEWAY-TLS tests pass without any patch.
No fix needed for now; revisit if a future conformance version adds strict
client-side certificate verification.

### 2.14 TLS backend pods serve stale certificates after Secret regeneration

**Symptom:** After framework recreates cert Secrets (new key pairs), TLS Passthrough
tests fail with `crypto/rsa: verification error`. The client has the new CA cert
but the backend pod still serves the old server cert.

**Root cause:** TLS backend pods (`tls-backend`, `tls-backend-2`, `tcp-backend`)
mount certificate Secrets as volumes and load them into memory at startup. Secret
updates on disk are not automatically reloaded by the `echo-basic` container.

**Affected deployments and their cert Secrets:**

| Namespace | Deployment | Secret |
|-----------|-----------|--------|
| `gateway-conformance-infra` | `tls-backend` | `tls-checks-certificate` |
| `gateway-conformance-infra` | `tls-backend-2` | `tls-checks-certificate` |
| `gateway-conformance-infra` | `tcp-backend` | `tls-passthrough-checks-certificate` |
| `gateway-conformance-app-backend` | `tls-backend` | `tls-passthrough-checks-certificate` |

**Fix:** `--clean-slate` now automatically performs `kubectl rollout restart` on all
TLS backend deployments before running tests. For manual recovery:

```bash
kubectl rollout restart deployment tls-backend -n gateway-conformance-infra
kubectl rollout restart deployment tls-backend-2 -n gateway-conformance-infra
kubectl rollout restart deployment tcp-backend -n gateway-conformance-infra
kubectl rollout restart deployment tls-backend -n gateway-conformance-app-backend
```

### 2.9 gRPC tests crash/hang via port-forward proxy

**Symptom:** `GRPCRouteWeight` test spams `DeadlineExceeded`, eventually the
`go test` process silently dies.

**Root cause:** gRPC (HTTP/2 long-lived streams) does not work reliably through
`kubectl port-forward`. The TAL gRPC proxy client encounters persistent
connection establishment failures.

**Fix:** Use in-cluster mode (see `02-conformance-testing.md`). If you must use
port-forward, do **not** set `GATEWAY_PROXY_ENABLE_GRPC=true`.

### 2.10 `kubectl exec` disconnects mid-test

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

### 2.11 Docker Hub rate limit on runner pod image pull

**Symptom:** Runner pod stuck in `ErrImagePull` / `ImagePullBackOff`.

**Fix:** Use an image already cached in the cluster:

```bash
image: pandaala/edgion-controller:conformance-dev
imagePullPolicy: IfNotPresent
```

### 2.12 Stale test resources from previous failed run

**Symptom:** `BackendTLSPolicy` reconcile sub-test fails — controller sets
`Accepted=True` when test expects `False`. Other tests may also see unexpected
pre-existing resources.

**Root cause:** `cleanup-base-resources=false` + test failure = cleanup callbacks
never fire. Dynamic resources (ConfigMap, BackendTLSPolicy, ReferenceGrant, etc.)
created mid-test survive into the next run. A stale ConfigMap with valid CA data
makes the controller resolve a reference that should be missing.

**Known offender:** `tls-checks-ca-certificate-reconcile-test` ConfigMap from
the `BackendTLSPolicy/Changing_the_content_of_a_ConfigMap` sub-test.

**Fix:**

```bash
# Quick — delete just the known stale CM
kubectl delete configmap tls-checks-ca-certificate-reconcile-test \
  -n gateway-conformance-infra --ignore-not-found

# Thorough — use --clean-slate to nuke all non-base resources before re-running
./scripts/in-cluster-runner.sh --run-only --clean-slate \
  --conformance-profiles "GATEWAY-HTTP" --run-test BackendTLSPolicy
```

**Prevention:** `in-cluster-runner.sh` now auto-deletes stale ConfigMaps before
every run. For a full reset use `--clean-slate` (deletes all non-base-manifest
resources across all conformance namespaces, then re-applies base manifests).

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
| `base-manifests-tls-only.yaml` | N/A | Filtered `base/manifests.yaml` with HTTP Gateways removed |

**Split-profile runs are mandatory.** HTTP/GRPC and TLS profiles cannot share
port 443. Gateway pod must be restarted after applying new manifests that add
ports (Edgion binds ports at startup only).

### Cert Secrets and Backend Pods

The conformance framework dynamically creates TLS Secrets at test setup time.
Key cert-to-deployment mappings:

| Secret | Used by (deployment) | Namespace |
|--------|---------------------|-----------|
| `tls-checks-certificate` | `tls-backend`, `tls-backend-2` | `gateway-conformance-infra` |
| `tls-passthrough-checks-certificate` | `tcp-backend` | `gateway-conformance-infra` |
| `tls-passthrough-checks-certificate` | `tls-backend` | `gateway-conformance-app-backend` |
| `tls-terminate-checks-certificate` | Gateway listeners (port 8443) | `gateway-conformance-infra` |

When Secrets are deleted/recreated, the corresponding backend pods **must** be
restarted to pick up new certs. `--clean-slate` does this automatically.

---

## 5. TLS Data Plane Quick Checklist

当 `TLSRoute` / TLS listener 相关用例失败时，按这个顺序排：

1. **看 Gateway / listener status**：是否 `Accepted=True`、`Programmed=True`，是否有 `ProtocolConflict` / `UnsupportedValue`
2. **看端口是否真的绑定**：在 gateway Pod 里检查 `443` / `8443` / `8883`
3. **看 `tls_access.log`**：连接有没有真的进代理，`err_log` 是 `no match`、`no listener accepts SNI` 还是 `no backend`
4. **看 gateway 结构化日志**：listener 初始化、证书加载、TLS callback 有没有报错
5. **看 `kubectl logs`**：有没有 crash-loop、`anyhow::bail!`、shell loop 的 `[restart]`
6. **看 `TLSRoute` status**：父引用、`Accepted`、`ResolvedRefs` 是否正确
7. **看 backend 健康度**：Service / EndpointSlice / Secret 是否齐全
8. **看 pre-bind manifest**：端口、hostname、TLS mode 是否和测试预期完全一致

常用命令：

```bash
GW_POD=$(kubectl get pods -n edgion-system -l app=edgion-gateway \
  -o jsonpath='{.items[0].metadata.name}')

kubectl get gateway -n gateway-conformance-infra
kubectl get tlsroute -n gateway-conformance-infra
kubectl exec -n edgion-system "$GW_POD" -- tail -20 /usr/local/edgion/logs/tls_access.log
kubectl exec -n edgion-system "$GW_POD" -- tail -50 /usr/local/edgion/logs/edgion-gateway.log
kubectl logs -n edgion-system "$GW_POD" --tail=50
```
