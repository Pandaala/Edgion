# Gateway API Conformance Testing Guide

## Quick Start

```bash
# Run HTTP Core (the most basic verification)
./scripts/run.sh --core

# Run a single test
./scripts/run.sh --run-test HTTPRouteSimpleSameNamespace

# Run Extended
./scripts/run.sh --extended
```

## TLSRoute-only environment

TLSRoute data-plane conformance requires the TLS passthrough listener on 443 to be
pre-created before the Gateway process starts. The default `conformance-gateway.yaml`
pre-creates an HTTPS 443 listener, which conflicts with the TLSRoute 443 passthrough listener.

Therefore, before running TLSRoute data-plane tests, use the TLS-only setup first:

```bash
./scripts/setup-orbstack.sh --tls-only
```

Then run the TLSRoute tests. Under OrbStack, `--port-forward` is usually not needed:

```bash
GO_RUN_FILTER="TestConformance/TLSRouteSimpleSameNamespace$" \
  bash scripts/run.sh --conformance-profiles GATEWAY-TLS
```

## Test granularity control

### Single test

```bash
./scripts/run.sh --run-test <TestName>
```

### By Conformance Profile

```bash
# Run only the HTTP profile
./scripts/run.sh --conformance-profiles GATEWAY-HTTP

# Run only the GRPC profile
./scripts/run.sh --conformance-profiles GATEWAY-GRPC \
  --supported-features "Gateway,GRPCRoute,ReferenceGrant"

# Run only the TLS profile
./scripts/run.sh --conformance-profiles GATEWAY-TLS \
  --supported-features "Gateway,TLSRoute,ReferenceGrant"

# Multiple profiles
./scripts/run.sh --conformance-profiles "GATEWAY-HTTP,GATEWAY-GRPC"
```

### By Feature subset

```bash
# Run only redirect-related tests
./scripts/run.sh --supported-features "Gateway,HTTPRoute,ReferenceGrant,HTTPRoutePathRedirect,HTTPRoutePortRedirect,HTTPRouteSchemeRedirect"

# Run only mirror-related tests
./scripts/run.sh --supported-features "Gateway,HTTPRoute,ReferenceGrant,HTTPRouteRequestMirror,HTTPRouteRequestMultipleMirrors"
```

### Using Go test regex matching

```bash
cd examples/gateway-api-conformance

# Match all tests whose name contains "Redirect"
go test ./... -run "TestConformance/.*Redirect" -v -timeout 10m -count=1 \
  -args -gateway-class=edgion -allow-crds-mismatch

# Match all tests whose name contains "TLSRoute"
go test ./... -run "TestConformance/TLSRoute" -v -timeout 10m -count=1 \
  -args -gateway-class=edgion -allow-crds-mismatch
```

### Preset modes

```bash
./scripts/run.sh --core          # Gateway + HTTPRoute + ReferenceGrant
./scripts/run.sh --extended      # Core + all implemented Extended features
./scripts/run.sh --all-features  # All features (including unimplemented ones)
```

## Transport Access Layer (TAL)

TAL is the unified access layer for Edgion conformance tests. It separates "how to reach the Gateway"
from "what protocol is being verified".

### How it works

TAL only rewrites the destination address of the underlying TCP connection; it does not modify any request semantics:
- Host header, `:authority`, URL path, TLS parameters, and request body are preserved as-is
- Address rewrites have optional debug logs: `TAL_DEBUG=true`

```
[TAL] resolved 192.168.194.236:80 -> 127.0.0.1:18080
```

### Covered protocol scenarios

| Scenario | Official extension point | TAL component |
|----------|--------------------------|----------------|
| HTTP/1.1 | `RoundTripper` | `tal.NewRoundTripper` |
| HTTPS / HTTP/2 over TLS | `RoundTripper` | `tal.NewRoundTripper` |
| H2C prior knowledge | `RoundTripper` | `tal.NewRoundTripper` |
| gRPC | `GRPCClient` | `tal.NewGRPCClient` |

### Known limits (issues TAL cannot solve)

- **WebSocket**: the official conformance framework has no dialer hook yet, so TAL cannot intercept it.
- **TLSRoute environment orchestration**: if the correct listener is not created before the Gateway starts, TAL cannot compensate.
- **Edgion data-plane bugs**: the request reached the Gateway but the response is wrong; this requires a product fix.

### Port-forward mode

When cluster Pod IPs are not directly reachable (remote clusters, etc.):

```bash
./scripts/run.sh --port-forward
./scripts/run.sh --port-forward --run-test HTTPRouteSimpleSameNamespace
```

> **Note for OrbStack users**: OrbStack natively supports direct macOS access to K8s ClusterIPs,
> so **do not add `--port-forward` for full test runs**. During long runs (~12 minutes) the
> port-forward can drop, leading to many `connection refused` errors. Just run `bash scripts/run.sh` directly.

You can also configure manually (the `GATEWAY_PROXY_PORT_MAP` unified port-mapping table is recommended):

```bash
export GATEWAY_PROXY_HOST=127.0.0.1
# Approach 1: unified port-mapping table (recommended)
export GATEWAY_PROXY_PORT_MAP="80:18080,443:18443,8443:28443"
# Approach 2: individual variables (compatible with the old style)
export GATEWAY_PROXY_HTTP_PORT=18080
export GATEWAY_PROXY_HTTPS_PORT=18443
export GATEWAY_PROXY_HTTPS_8443_PORT=28443

kubectl port-forward -n edgion-system pod/<gateway-pod> 18080:80 18443:443 28443:8443 &
./scripts/run.sh --run-test HTTPRouteSimpleSameNamespace
```

Enable the gRPC proxy (the `GRPCRoute*` test family):

```bash
GATEWAY_PROXY_ENABLE_GRPC=true ./scripts/run.sh --port-forward --conformance-profiles GATEWAY-GRPC
```

## All options

```
--gateway-class <name>        GatewayClass name (default: edgion)
--supported-features <list>   Comma-separated feature list
--conformance-profiles <list> Comma-separated profile list
--run-test <name>             Run a single test
--all-features                Enable all features
--core                        Core features only
--extended                    Core + Extended features
--cleanup                     Clean up after tests
--no-debug                    Disable debug output
--timeout <duration>          Timeout (default: 30m)
--report <path>               Report output path
--port-forward                Auto port-forward
```

## Conformance Profiles

All three profiles are declared in `conformance_test.go` (`opts.ConformanceProfiles`).

| Profile | Declared | Core | Extended | Notes |
|---------|----------|------|----------|-------|
| GATEWAY-HTTP | OK | 33/33 passed | 24/28 passed | Core fully passes; Extended has 4 failures (mTLS/421) |
| GATEWAY-GRPC | OK | 9/13 passed | — | 4 failures: gRPC returns Internal instead of Unimplemented when no route matches |
| GATEWAY-TLS | OK | 14/18 passed | — | 4 failures: dynamic TLS passthrough listener is not supported |

> Data as of: 2026-03-29, full run `bash scripts/run.sh` (direct connection, no port-forward).

## Progress tracking

Per-test analyses of failures are under: `tasks/todo/gateway-api-1.5-conformance-tests/`.

One file per failing test, including the error log, root cause analysis, fix direction, and reproduction commands.
