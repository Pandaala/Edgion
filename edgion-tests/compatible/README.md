# Gateway API CRD compatibility tests

`run_compatible_test.sh` installs exact Gateway API release CRDs in a dedicated
Kubernetes cluster and tests the current Edgion Controller and Gateway images.
The release window starts with v1.0.0 (released 2023-10-31) and ends with
v1.6.2 (released 2026-09-03). Each stable release has its own `v*/case.json`
directory. Only the Standard CRD channel is supported and tested. The
official release URLs and SHA-256 values are locked in
`../gateway_api_bundles/artifacts.json`; a digest mismatch fails before apply.

## Run

Use a disposable Kubernetes cluster with no Edgion namespaces, Edgion CRDs,
or Gateway API CRDs. The runner checks this before changing the cluster and
requires the named context to be the active context. It deletes its namespaces,
RBAC, and CRDs between cases; do not use a shared cluster.

```bash
python3 -m pip install -r edgion-tests/gateway_api_bundles/requirements.txt
./edgion-tests/compatible/run_compatible_test.sh \
  --context kind-compatible \
  --controller-image registry.example.com/edgion-controller:current \
  --gateway-image registry.example.com/edgion-gateway:current \
  --test-server-image registry.example.com/edgion-test-server:current
```

Images must be pullable by the target cluster. The test server image defaults
to the image in the Kubernetes fixture Deployment if no override is given.
The runner also uses `python:3.12-alpine` for a temporary in-cluster
HTTP/TCP/UDP/TLS probe. Use `--version v1.0.0` to narrow a run.

An existing Edgion runtime with no Gateway API resource objects can be tested
with `--reuse-cluster`. This mode snapshots the Gateway API CRDs, Controller
configuration, Deployment images, and replica counts; scales Controller and
Gateway down while replacing CRDs; then restores the snapshot on exit. It
requires the test namespaces to be absent and leaves existing Edgion CRDs,
RBAC, and the `edgion-system` namespace untouched. The cluster is unavailable
for ordinary Gateway traffic during the matrix.
Results and failure logs are written under `tmp/compatible_testing/` by
default; `--report-dir` changes that location. Every Standard release
gets a `result.json`. Any failed release returns a nonzero exit code. A cleanup
residue stops the matrix before the next CRD version is installed.

The read-only artifact, source-inventory, and fixture OpenAPI gate runs without
a cluster:

```bash
./edgion-tests/compatible/run_compatible_test.sh --check-artifacts
```

Each `v*/standard/` directory contains a `suite.yaml` inventory and one YAML
resource file per kind supported by Edgion in that exact Standard bundle.
The inventory lists every tested source GVK and the unavailable kinds the
Controller must explicitly skip. The runner applies those YAML files directly
and refuses a missing, unserved, or unlisted source.

## Standard resource coverage

Each cell lists the source API versions exercised by that release's YAML suite.
`v1 + beta` means both `v1` and `v1beta1`; `beta` means `v1beta1` only.
An em dash means that the Standard bundle does not provide a source version
supported by Edgion. The exact GVK inventory is in each `standard/suite.yaml`.

| Gateway API release | GatewayClass | Gateway | HTTPRoute | ReferenceGrant | GRPCRoute | TCPRoute | UDPRoute | TLSRoute | BackendTLSPolicy |
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

## Assertions

For each Standard release, the runner installs only the CRD documents from
the locked official artifact, then the current Edgion CRDs and minimal K8s
fixtures. It waits for the Controller, Gateway, and backend Deployments. It
creates `GatewayClass/v1`, `Gateway/v1`, and `HTTPRoute/v1`, checks Accepted
status, and sends HTTP traffic through the Gateway to the test Service.
When the bundle serves the v1beta1 core sources, a second GatewayClass,
Gateway, and HTTPRoute are submitted as v1beta1 and checked through a
separate HTTP listener. The v1beta1 ReferenceGrant source is submitted where
it is served.
When the installed CRD serves them, it also creates and checks the implemented
GRPCRoute, TCPRoute, UDPRoute, and TLSRoute source versions. TCP, UDP, and
TLSRoute also forward probe traffic to the test backend. It exercises
ReferenceGrant with a cross-namespace HTTPRoute and a second HTTP request,
and checks BackendTLSPolicy status. Unsupported older served versions such as
TLSRoute/v1alpha2 remain outside Edgion's source-version contract; the result
file records them as unavailable. The source selection follows
`edgion-resources/src/gateway_api_support.rs`.

Accepted status on GRPCRoute or BackendTLSPolicy is an attachment and
reconciliation check, not a data-plane traffic assertion for that kind. The
existing K8s integration and Gateway API conformance suites remain necessary
for feature and protocol behavior. A passing compatibility run applies to the
tested images, Kubernetes version, Standard CRD release, and fixtures; it does
not establish support for every field in a served GVK.
