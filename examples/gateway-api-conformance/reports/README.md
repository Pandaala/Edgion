# Gateway API Conformance Test Reports

Reports generated on **2026-04-05** against Edgion `dev` build.

## Report Files

| File | Profiles | Date | Notes |
|------|----------|------|-------|
| `full-conformance-report.yaml` | GATEWAY-HTTP, GATEWAY-GRPC, GATEWAY-TLS | 2026-04-05T11:06:26Z | Full run (all profiles together) |
| `gateway-tls-conformance-report.yaml` | GATEWAY-TLS | 2026-04-05T11:19:55Z | TLS only, but with HTTPS listeners still bound on port 443 |
| `gateway-tls-standalone-report.yaml` | GATEWAY-TLS | 2026-04-05T12:46:27Z | TLS standalone: gateway restarted with TLS pre-bind only, no HTTPS on 443 |

## Results Summary

### GATEWAY-HTTP — PASS

- **Core**: 33/33 passed
- **Extended**: 32/32 passed

### GATEWAY-GRPC — PASS

- **Core**: 13/13 passed

### GATEWAY-TLS — Standalone Run

- **Core**: 17/18 passed, 1 failed
- **Extended**: 2/3 passed, 1 failed

## GATEWAY-TLS Failure Analysis

### Root Cause: Port 443 Listener Conflict (in full run)

When running GATEWAY-TLS together with GATEWAY-HTTP, the base manifest creates
`same-namespace-with-https-listener` Gateway on port 443 with HTTPS (Terminate)
protocol. Since Edgion binds physical listeners at startup, this HTTPS listener
occupies port 443 before TLS Passthrough test Gateways are created. TLS Passthrough
requests then hit the HTTPS listener, receiving incorrect certificates (`*.org`,
`*.wildcard.org` instead of `abc.example.com`).

**Fix**: Run TLS tests with a dedicated startup sequence:
1. Apply `manifests/conformance-gateway-tls.yaml` (pre-binds port 443 as TLS Passthrough)
2. Restart gateway deployment
3. Run GATEWAY-TLS conformance

This reduced failures from **5 (full run)** to **2 (standalone)**.

### Remaining Failures

| Test | Tier | Root Cause |
|------|------|------------|
| `TLSRouteHostnameIntersection` | Core | Backend port mapping bug: `tls-backend` Service has `port:443 → targetPort:8443`, but TLS Passthrough proxy connects to Pod IP on port 443 (which is not listening). The `resolve_target_port` function skips port mapping for single-port Services, but the roundrobin backend store appears to use the Service port (443) instead of the EndpointSlice targetPort (8443). |
| `TLSRouteTerminateSimpleSameNamespace` | Extended | Configuration sync latency: the dynamically-created `gateway-tlsroute-terminate` Gateway's TLS certificate takes >30s to propagate through controller → gateway sync, exceeding the test's 30s timeout. The test log shows "Request passed" at the 30s mark, confirming the request succeeds eventually. |

### Tests Fixed by Standalone TLS Run

| Test | Before | After | Fix |
|------|--------|-------|-----|
| `TLSRouteSimpleSameNamespace` | FAIL | PASS | Port 443 now bound as TLS Passthrough instead of HTTPS |
| `TLSRouteInvalidBackendRefNonexistent` | FAIL | PASS | Same — connection correctly rejected instead of hitting HTTPS listener |
| `TLSRouteInvalidBackendRefUnknownKind` | FAIL | PASS | Same |
