---
name: conformance-debug-cookbook
description: Conformance test troubleshooting recipes — pre-flight checks, common failure patterns, and fixes derived from real debugging sessions.
---

# Conformance Debug Cookbook

Run pre-flight checks (§1) before every suite. Use §2 when a test fails or
behaves unexpectedly — each entry follows the pattern: Symptom → Root cause → Fix.

## 1. Pre-flight Checks

Run these **before** starting any conformance suite. Most "stuck" runs trace
back to a skipped pre-flight item.

### 1.1 Controller healthy

```bash
POD=$(kubectl get pods -n edgion-system -l app=edgion-controller \
  -o jsonpath='{.items[0].metadata.name}')
kubectl logs --tail=20 -n edgion-system "$POD" | grep -iE 'listen|started|error'
```

Expect: gRPC server listening + reconcile loop started, zero `ERROR`/`panic` lines.

### 1.2 Gateway healthy

```bash
POD=$(kubectl get pods -n edgion-system -l app=edgion-gateway \
  -o jsonpath='{.items[0].metadata.name}')
kubectl logs --tail=30 -n edgion-system "$POD" | grep -iE 'bindto|bindaddr|listen|error'
```

Expect: one `bindto`/`listen` line per expected port (80, 443, 8443…). A missing
port means those tests will time out with `connection refused`.

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
kubectl apply -f edgion-tests/conformance/manifests/conformance-gateway.yaml
kubectl apply -f edgion-tests/conformance/manifests/conformance-gateway-tls.yaml
```

Verify key Gateways exist:

```bash
kubectl get gateways -n gateway-conformance-infra
# Expected: same-namespace, same-namespace-with-https-listener, all-namespaces,
#           gateway-tlsroute, pre-bind-tls-terminate-8443, pre-bind-tls-mixed-8883, ...
```

### 1.5 No stale processes

```bash
# In-cluster runner pod
kubectl exec -n edgion-system conformance-runner -- ps aux 2>/dev/null | grep conformance-tes
# Local
pgrep -f "gateway-api-conformance.test" && echo "STALE — kill first"
```

Kill any leftover `<defunct>` or lingering test processes before starting.

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
kubectl apply -f edgion-tests/conformance/manifests/conformance-gateway.yaml
kubectl apply -f edgion-tests/conformance/manifests/conformance-gateway-tls.yaml
kubectl rollout restart deployment/edgion-gateway -n edgion-system
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
kubectl apply -f edgion-tests/conformance/manifests/conformance-gateway.yaml
kubectl apply -f edgion-tests/conformance/manifests/conformance-gateway-tls.yaml
```

Then restart the gateway pod to pick up the new ports.

**Impact:** ~100+ HTTP sub-tests × 60s timeout = 2+ hour hang. Reducing
`MaxTimeToConsistency` to 15s mitigates but does not fix the root cause.

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
- Secret `tls-terminate-checks-certificate` missing in `gateway-conformance-infra`
  — created dynamically by the suite; gone if namespace was cleaned up.
- Gateway restarted before the Secret was re-created.

### 2.3.1 Port 443 mixed-mode conflict (HTTPS Terminate + TLS Passthrough)

**Architectural constraint:** Edgion does **not** support mixing HTTPS Terminate
and TLS Passthrough on the same port. Conformance tests hardcode port 443 for
both profiles with no way to remap. GATEWAY-HTTP and GATEWAY-TLS **must be run
separately** — TLS Passthrough requests on 443 would otherwise be intercepted by
the HTTPS Terminate listener, returning the wrong certificate or timing out.

**Affected tests when run together (all TLSRoute data-plane tests):**
`TLSRouteSimpleSameNamespace`, `TLSRouteHostnameIntersection`,
`TLSRouteTerminateSimpleSameNamespace`, `TLSRouteInvalidBackendRefNonexistent`,
`TLSRouteInvalidBackendRefUnknownKind`. Status-only TLSRoute tests pass fine.

**Run protocol:**

`in-cluster-runner.sh --clean-slate` handles everything automatically (deletes
non-base resources, applies profile-specific manifests, restarts processes,
saves reports to `reports/`):

```bash
# Step 1: GATEWAY-HTTP + GATEWAY-GRPC
./scripts/in-cluster-runner.sh --clean-slate \
  --conformance-profiles "GATEWAY-HTTP,GATEWAY-GRPC" --timeout 30m

# Step 2: GATEWAY-TLS only
./scripts/in-cluster-runner.sh --clean-slate \
  --conformance-profiles "GATEWAY-TLS" --timeout 30m
```

For TLS runs, `base-manifests-tls-only.yaml` is a filtered copy of upstream
`base/manifests.yaml` with HTTP Gateways removed to prevent the port 443
conflict. `--clean-slate` also restarts TLS backend deployments — cert Secrets
are regenerated with new key pairs and pods cache the old ones. See §2.14.

### 2.3.2 Client-cert validation Gateways pollute port 443

**Symptom:** `HTTPRouteHTTPSListener` (GATEWAY-HTTP core) fails with
`tls: certificate required` on every HTTPS request to port 443.

**Root cause:** `client-validation-default` and
`client-validation-insecure-fallback` Gateways (for
`GatewayFrontendClientCertificateValidation`, which Edgion does **not** declare)
had HTTPS Terminate listeners on port 443 requiring client certs, intercepting
all port-443 HTTPS traffic.

**Fix:** Removed from `conformance-gateway.yaml`. If the feature is declared
in future, use a dedicated port (not 443) to avoid conflicting with
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

**Root cause:** `edgion-controller/src/cli/mod.rs` started the 25s shutdown timer
at service boot, not at signal receipt, causing the controller to exit 25s after
start regardless of workload.

**Fix (applied 2026-04-07):** Replaced with `tokio::select!` — drain deadline
now starts only after `ShutdownHandle::signal().wait()` fires.

### 2.13 TLS cert x509 CA:FALSE — Go 1.24+ (not triggered in v1.5.1)

**Symptom:** All TLS data-plane tests fail with `tls: failed to verify certificate:
x509: certificate signed by unknown authority (possibly because of "x509: invalid
signature: parent certificate cannot sign this kind of certificate")`.

**Root cause:** `sigs.k8s.io/gateway-api/conformance v1.5.1` generates
self-signed certs with `BasicConstraints CA:FALSE`; Go 1.24+ rejects these via
`x509.Verify`.

**Status (verified 2026-04-09):** v1.5.1 tests use `InsecureSkipVerify` — this
code path is never triggered. All 21 GATEWAY-TLS tests pass without any patch.
Revisit if a future conformance version adds strict client-side verification.

### 2.14 TLS backend pods serve stale certificates after Secret regeneration

**Symptom:** After framework recreates cert Secrets (new key pairs), TLS Passthrough
tests fail with `crypto/rsa: verification error`. The client has the new CA cert
but the backend pod still serves the old server cert.

**Root cause:** TLS backend pods (`tls-backend`, `tls-backend-2`, `tcp-backend`)
mount certificate Secrets as volumes and load them into memory at startup. Secret
updates on disk are not automatically reloaded by the `echo-basic` container.
Cert-to-deployment mapping is in §4.

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
`kubectl port-forward` — the TAL gRPC proxy client encounters persistent
connection establishment failures.

**Fix:** Use in-cluster mode (`02-conformance-testing.md`). If you must use
port-forward, do **not** set `GATEWAY_PROXY_ENABLE_GRPC=true`.

### 2.10 `kubectl exec` disconnects mid-test

**Symptom:** `kubectl exec` returns while the test is still running inside the
pod; log file on the local machine is truncated.

**Root cause:** `kubectl exec` has idle/connection timeouts; long-running output
piped via `kubectl exec > local-file` can lose data when the connection drops.

**Fix:** Use `nohup` inside the pod to write logs to a pod-local file, then
retrieve with `kubectl cp`:

```bash
kubectl exec -n edgion-system conformance-runner -- sh -c '
nohup /tmp/conformance-test -test.run=TestConformance -test.v ... \
  > /tmp/conformance.log 2>&1 & echo "PID=$!"'
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
never fire. Dynamic resources survive into the next run. Known offender:
`tls-checks-ca-certificate-reconcile-test` ConfigMap from the
`BackendTLSPolicy/Changing_the_content_of_a_ConfigMap` sub-test.

**Fix:**

```bash
kubectl delete configmap tls-checks-ca-certificate-reconcile-test \
  -n gateway-conformance-infra --ignore-not-found
```

For a full reset, use `--clean-slate` (deletes all non-base-manifest resources
across all conformance namespaces, then re-applies base manifests).
`in-cluster-runner.sh` now auto-deletes stale ConfigMaps before every run.

---

## 3. Run Efficiency Tips

### 3.1 In-cluster mode (recommended)

Avoids all OrbStack routing / port-forward / gRPC proxy issues. Full steps in
`02-conformance-testing.md` "In-cluster execution" section.

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

**Split-profile runs are mandatory** (see §2.3.1). Gateway pod must be restarted
after applying new manifests that add ports — Edgion binds ports at startup only.

### Cert Secrets and Backend Pods

Cert-to-deployment mappings (Secrets are dynamically created by the framework):

| Secret | Used by (deployment) | Namespace |
|--------|---------------------|-----------|
| `tls-checks-certificate` | `tls-backend`, `tls-backend-2` | `gateway-conformance-infra` |
| `tls-passthrough-checks-certificate` | `tcp-backend` | `gateway-conformance-infra` |
| `tls-passthrough-checks-certificate` | `tls-backend` | `gateway-conformance-app-backend` |
| `tls-terminate-checks-certificate` | Gateway listeners (port 8443) | `gateway-conformance-infra` |

When Secrets are deleted/recreated, backend pods **must** be restarted.
`--clean-slate` does this automatically (see §2.14).

---

## 5. TLS Data Plane Quick Checklist

When `TLSRoute` / TLS-listener test cases fail, work through this in order:

1. **Check Gateway / listener status**: is `Accepted=True`, `Programmed=True`; any `ProtocolConflict` / `UnsupportedValue`?
2. **Check whether the port is actually bound**: inside the gateway pod, verify `443` / `8443` / `8883`
3. **Check `tls_access.log`**: did the connection actually reach the proxy; is `err_log` `no match`, `no listener accepts SNI`, or `no backend`?
4. **Check the gateway structured logs**: any errors during listener init, certificate load, or TLS callback?
5. **Check `kubectl logs`**: any crash-loops, `anyhow::bail!`, or `[restart]` lines from the shell loop?
6. **Check `TLSRoute` status**: parent refs, `Accepted`, `ResolvedRefs` correct?
7. **Check backend health**: are Service / EndpointSlice / Secret all present?
8. **Check the pre-bind manifest**: do the port, hostname, and TLS mode exactly match what the test expects?

Common commands (`GW_POD` from §1.2):

```bash
kubectl get gateway -n gateway-conformance-infra
kubectl get tlsroute -n gateway-conformance-infra
kubectl exec -n edgion-system "$GW_POD" -- tail -20 /usr/local/edgion/logs/tls_access.log
kubectl exec -n edgion-system "$GW_POD" -- tail -50 /usr/local/edgion/logs/edgion-gateway.log
kubectl logs -n edgion-system "$GW_POD" --tail=50
```

## Related

- Per-profile failure modes via the rv/sv/access-log tracing chain + `orbstack-dev.sh` log commands: [`../../09-misc/debugging-conformance.md`](../../09-misc/debugging-conformance.md)
- Cross-test leakage / timeline correlation: [`05-batch-conformance-debugging.md`](05-batch-conformance-debugging.md)
