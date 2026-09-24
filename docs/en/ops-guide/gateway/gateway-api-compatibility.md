# Gateway API Standard CRD compatibility

Edgion tests the **Standard** Gateway API CRD bundles released from v1.0.0
through v1.6.2. The Experimental bundle is outside this compatibility scope.
A bundle release (for example, v1.5.0) and a resource API version (for
example, `gateway.networking.k8s.io/v1/HTTPRoute`) are different things: the
same resource version can be served by several bundle releases.

## Tested resource versions by bundle

Each cell is a resource source version submitted by the compatibility YAML
suite for that exact Standard bundle. `v1 + beta` means both `v1` and
`v1beta1`; `beta` means `v1beta1` only. A dash means the Standard bundle has
no Edgion-supported source version for that resource. The corresponding YAML
files and exact GVK inventory are in each
[`edgion-tests/compatible/v*/standard/` suite](../../../../edgion-tests/compatible/README.md).

| Standard bundle | GatewayClass | Gateway | HTTPRoute | ReferenceGrant | GRPCRoute | TCPRoute | UDPRoute | TLSRoute | BackendTLSPolicy |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| v1.0.0 | v1 + beta | v1 + beta | v1 + beta | beta | — | — | — | — | — |
| v1.1.0 | v1 + beta | v1 + beta | v1 + beta | beta | v1 | — | — | — | — |
| v1.1.1 | v1 + beta | v1 + beta | v1 + beta | beta | v1 | — | — | — | — |
| v1.2.0 | v1 + beta | v1 + beta | v1 + beta | beta | v1 | — | — | — | — |
| v1.2.1 | v1 + beta | v1 + beta | v1 + beta | beta | v1 | — | — | — | — |
| v1.3.0 | v1 + beta | v1 + beta | v1 + beta | beta | v1 | — | — | — | — |
| v1.4.0 | v1 + beta | v1 + beta | v1 + beta | beta | v1 | — | — | — | v1 |
| v1.4.1 | v1 + beta | v1 + beta | v1 + beta | beta | v1 | — | — | — | v1 |
| v1.5.0 | v1 + beta | v1 + beta | v1 + beta | v1 + beta | v1 | — | — | v1 | v1 |
| v1.5.1 | v1 + beta | v1 + beta | v1 + beta | v1 + beta | v1 | — | — | v1 | v1 |
| v1.6.0 | v1 + beta | v1 + beta | v1 + beta | v1 + beta | v1 | v1 | v1 | v1 | v1 |
| v1.6.1 | v1 + beta | v1 + beta | v1 + beta | v1 + beta | v1 | v1 | v1 | v1 | v1 |
| v1.6.2 | v1 + beta | v1 + beta | v1 + beta | v1 + beta | v1 | v1 | v1 | v1 | v1 |

## What the test proves

For each row, `run_compatible_test.sh` installs the pinned official Standard
CRDs, starts the Edgion Controller and Gateway, applies the listed YAML
resources, and waits for their `Accepted=True` status. It probes HTTP routing,
cross-namespace access through ReferenceGrant, and the v1beta1 HTTP route.
Where both ReferenceGrant source versions are served, each authorizes a
different backend Service and both cross-namespace requests are checked.
Where those resources are available, the runner also probes unary gRPC health,
TCP, UDP, and TLS passthrough forwarding. BackendTLSPolicy is checked against
a private-CA HTTPS backend: a matching CA and hostname must succeed, while an
incorrect validation hostname must fail. The suite also validates its YAML
against each bundle's OpenAPI schema before a cluster run.

All 13 Standard releases passed the compatibility suite on an OrbStack
Kubernetes v1.33.9 arm64 cluster on 2026-09-24. This is a tested resource and
basic behavior matrix, not full Gateway API conformance or a promise that
every optional field works. Check each feature's documentation and the
[resource-version boundaries](../../dev-guide/gateway-api-version-support.md)
before using it.

## Run the matrix

Use a disposable cluster, images that it can pull, and `grpcurl` installed on
the runner host:

```bash
python3 -m pip install -r edgion-tests/gateway_api_bundles/requirements.txt
./edgion-tests/compatible/run_compatible_test.sh \
  --context kind-compatible \
  --controller-image registry.example.com/edgion-controller:current \
  --gateway-image registry.example.com/edgion-gateway:current \
  --test-server-image registry.example.com/edgion-test-server:current
```

The runner also supports `--version v1.5.0` for one release and
`--check-artifacts` for a cluster-free CRD and YAML schema check. See the
[test runner README](../../../../edgion-tests/compatible/README.md) for the
cluster prerequisites and `--reuse-cluster` behavior.
