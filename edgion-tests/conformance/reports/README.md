# Gateway API Conformance Test Reports

The 2026-06 reports are historical Edgion `dev` runs. They record
`gatewayAPIVersion: UNDEFINED` and `gatewayAPIChannel: UNDEFINED`, so they
cannot establish compatibility with a particular installed CRD bundle.
The current Go module pins both `sigs.k8s.io/gateway-api` and
`sigs.k8s.io/gateway-api/conformance` at v1.6.2. These updated dependencies do
not recover the missing bundle/channel metadata from an old run.

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
| `gateway-http-grpc-conformance-report.yaml` | GATEWAY-HTTP, GATEWAY-GRPC | 2026-06-18 13:54:50 UTC | Split-profile run |
| `gateway-tls-conformance-report.yaml` | GATEWAY-TLS | 2026-06-18 13:17:59 UTC | Split-profile run with TLS-only base manifest |
| `gateway-http-conformance-report.yaml` | GATEWAY-HTTP | 2026-05-23 | Earlier partial/failing report, retained as history |
| `gateway-http-grpc-v1.6.2-kind-2026-09-23.yaml` | GATEWAY-HTTP, GATEWAY-GRPC | 2026-09-23 | v1.6.2 Experimental, isolated kind node run; all selected tests passed |
| `gateway-tls-v1.6.2-kind-2026-09-23.yaml` | GATEWAY-TLS | 2026-09-23 | v1.6.2 Experimental, isolated kind node run; one extended test failed |
| `gateway-http-grpc-v1.6.2-standard-kind-2026-09-24.yaml` | GATEWAY-HTTP, GATEWAY-GRPC | 2026-09-24 | v1.6.2 Standard; HTTP Core 36/37, Extended 26/30, GRPC Core 15/15 |
| `gateway-tls-v1.6.2-standard-kind-2026-09-24.yaml` | GATEWAY-TLS | 2026-09-24 | v1.6.2 Standard; TLS Core 20/20 |

## Standard v1.6.2 result boundary

The Standard runs used an isolated kind cluster on OrbStack and local arm64
Controller and Gateway images. The unmodified upstream HTTP/GRPC suite failed
`HTTPRouteMultipleGateways`: the four base Gateways all published the same
Gateway Service IP, so identical host/path requests could not select a
distinct Gateway parent. Four HTTP Extended tests also failed in the combined
run; a targeted `BackendTLSPolicySANValidation` run passed, but that does not
establish a full-profile pass. GRPC Core passed 15/15.

The TLS-only run used the unmodified upstream test binary with the pinned
TLS-only base manifest and passed Core 20/20. It does not report an Extended
profile. TCP and UDP remain outside the declared feature set because dynamic
listener binding is not implemented.

An addressed-hostNetwork experiment was excluded from these reports: it
assigned node loopback IPs to the test Gateways and patched upstream fixture
preparation to preserve them. Normal Gateway Pods use their own network
namespace behind a Service, so a Gateway address or Service VIP need not be
present on a Pod. That experiment does not establish support for binding
`Gateway.spec.addresses` in the default Kubernetes deployment.

The new reports use Go conformance v1.6.2 and Gateway API v1.6.2
Experimental CRDs with local arm64 Edgion images. The full HTTP profile
passed Core 37/37 and Extended 30/30; GRPC passed Core 15/15. The full TLS
profile passed Core 20/20 and Extended 2/3. Its only failure was
`TLSRouteTerminateSimpleSameNamespace` after a separate targeted pass. The TLS
full-profile follow-up was canceled; `TLSRouteModeTerminate` remains unsupported.
The TCP and UDP full profiles timed out while probing listener ports created
after Gateway startup, so neither produced a report. Dynamic TCP/UDP listener
support was declined; port changes require a Gateway process restart. See the
[accepted listener boundary](../../../skills/04-review/listeners/compatibility-constraints/listener-port-set-frozen-at-startup.md).

## Historical run commands

```bash
# Step 1: HTTP + gRPC
./scripts/in-cluster-runner.sh --clean-slate \
  --conformance-profiles "GATEWAY-HTTP,GATEWAY-GRPC" --timeout 30m

# Step 2: TLS (requires separate run — port 443 conflict)
./scripts/in-cluster-runner.sh --clean-slate \
  --conformance-profiles "GATEWAY-TLS" --timeout 30m
```

These commands describe the historical split-profile setup. A new run uses
the then-current code and installed CRDs; it cannot recreate the undefined
bundle/channel metadata in the older reports.

## Results Summary

### GATEWAY-HTTP — 2026-06-18 report

- **Core**: 33/33 passed
- **Extended**: 31/31 passed

### GATEWAY-GRPC — 2026-06-18 report

- **Core**: 13/13 passed

### GATEWAY-TLS — 2026-06-18 report

- **Core**: 18/18 passed
- **Extended**: 3/3 passed (`TLSRouteModeTerminate`, `GatewayHTTPListenerIsolation`, `GatewayHTTPSListenerDetectMisdirectedRequests`)

The earlier GATEWAY-HTTP report dated 2026-05-23 recorded core 0 passed,
33 skipped, and extended 0 passed, 1 failed, 31 skipped. It is retained so
the later improvement can be audited. Neither report series has a defined
installed bundle version or channel, and no test result here is assigned to
a specific source GVK or release in the offline fixture matrix.

## Known Quirks

### Historical certificate issue

An earlier Go toolchain and conformance suite combination exposed a self-signed
certificate CA constraint issue. The current v1.6.2 module has no local
conformance replacement. The saved reports predate this module update.

### TLS backend pods must restart after cert regeneration

TLS backend pods (`tls-backend`, `tls-backend-2`, `tcp-backend`) mount certificate
Secrets as volumes and load them once at startup. When the conformance framework
regenerates Secrets with new key pairs, running pods still serve old certificates.
`--clean-slate` handles this automatically by rolling-restarting all TLS backend
deployments before running tests.
