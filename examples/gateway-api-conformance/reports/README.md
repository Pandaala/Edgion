# Gateway API Conformance Test Reports

Reports generated on **2026-04-07** against Edgion `dev` build.

## Why Split-Profile Runs

Edgion does **not** support HTTPS Terminate and TLS Passthrough on the same port.
The upstream conformance tests hardcode port 443 for both modes. Running all
profiles together causes TLS Passthrough requests to be intercepted by the HTTPS
Terminate listener, producing certificate mismatches and test failures.

Therefore GATEWAY-HTTP/GRPC and GATEWAY-TLS **must be run separately**, each
with its own base manifest set and gateway startup configuration:

- **HTTP/GRPC run**: uses upstream `base/manifests.yaml` (HTTP Gateways on 80/443)
- **TLS run**: uses `base-manifests-tls-only.yaml` (HTTP Gateways removed) +
  `conformance-gateway-tls.yaml` (TLS pre-bind Gateways on 443/8443/8883)

## Report Files

| File | Profiles | Date | Notes |
|------|----------|------|-------|
| `gateway-http-grpc-conformance-report.yaml` | GATEWAY-HTTP, GATEWAY-GRPC | 2026-04-07 | Split-profile run |
| `gateway-tls-conformance-report.yaml` | GATEWAY-TLS | 2026-04-07 | Split-profile run with TLS-only base manifest |

## How to Reproduce

```bash
# Step 1: HTTP + gRPC
./scripts/in-cluster-runner.sh --clean-slate \
  --conformance-profiles "GATEWAY-HTTP,GATEWAY-GRPC" --timeout 30m

# Step 2: TLS (requires separate run — port 443 conflict)
./scripts/in-cluster-runner.sh --clean-slate \
  --conformance-profiles "GATEWAY-TLS" --timeout 30m
```

Reports are automatically saved to this directory with profile-derived names.

## Results Summary

### GATEWAY-HTTP — FULL PASS

- **Core**: 33/33 passed
- **Extended**: 32/32 passed (including `BackendTLSPolicyConflictResolution`)

### GATEWAY-GRPC — FULL PASS

- **Core**: 13/13 passed

### GATEWAY-TLS — FULL PASS

- **Core**: 18/18 passed
- **Extended**: 3/3 passed (`TLSRouteModeTerminate`, `GatewayHTTPListenerIsolation`, `GatewayHTTPSListenerDetectMisdirectedRequests`)

## Known Quirks

### Upstream conformance cert CA:FALSE (Go 1.24+)

The upstream conformance suite (`sigs.k8s.io/gateway-api/conformance v1.5.1`)
generates self-signed certificates with `BasicConstraints CA:FALSE`. Go 1.24+
rejects these when used as root CAs. Edgion patches this locally via a `replace`
directive in `go.mod` pointing to `patches/conformance-v1.5.1/`, which sets
`IsCA=true` and adds `KeyUsageCertSign` for self-signed certs.

### TLS backend pods must restart after cert regeneration

TLS backend pods (`tls-backend`, `tls-backend-2`, `tcp-backend`) mount certificate
Secrets as volumes and load them once at startup. When the conformance framework
regenerates Secrets with new key pairs, running pods still serve old certificates.
`--clean-slate` handles this automatically by rolling-restarting all TLS backend
deployments before running tests.
