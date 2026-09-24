# Gateway API v1.6.2 TLS termination full-profile conformance

## Current state

The isolated v1.6.2 Experimental `GATEWAY-TLS` run passed Core 20/20 and
Extended 2/3. `TLSRouteTerminateSimpleSameNamespace` passed when run alone but
timed out in the full profile. The saved result is in
[`gateway-tls-v1.6.2-kind-2026-09-23.yaml`](../../../edgion-tests/conformance/reports/gateway-tls-v1.6.2-kind-2026-09-23.yaml).
Edgion therefore does not advertise `TLSRouteModeTerminate` in
`GatewayClass.status.supportedFeatures`.

## Follow-up

1. Reproduce the full-profile timeout in an isolated cluster with the pinned
   v1.6.2 Experimental CRDs and Go conformance suite.
2. Identify and fix the cause while preserving the separate HTTP/GRPC and TLS
   profile runs required by their port 443 listener conflict.
3. Rerun the complete `GATEWAY-TLS` profile and save a passing report before
   advertising `TLSRouteModeTerminate`.
