# K8s Integration Test Entry

This directory is the Kubernetes test entry for Edgion examples.

## Layout

- `conf/`: Kubernetes-oriented test manifests converted from `edgion-tests/integration/conf`.
- `scripts/`: K8s integration runner and helpers.
- `kubernetes/`: self-contained deployment manifests for controller/gateway/test workloads.

## Usage

### Prerequisites

The runner targets a standard Kubernetes API and does not require OrbStack.
Before it mutates the cluster it automatically verifies:

- `kubectl` has a current context and can reach the API server;
- the context contains at least one Ready Node with a reported architecture;
- `kubectl`, `jq`, `curl`, `python3`, and PyYAML are installed.

On a machine with multiple contexts, pin the intended target without making
any distribution name mandatory:

```bash
EDGION_K8S_EXPECTED_CONTEXT=my-test-cluster \
  ./edgion-tests/k8s/scripts/run_k8s_integration.sh ...
```

The runner fails before mutation if the active context differs.

Docker is required only for local image modes that build images
(`build-import` and `prepare-local-images`). A remote cluster may use any
registry-visible multi-architecture images without Docker on the runner host.

For an unfiltered run, the image config must set all five images, including
the dedicated isolated-job image:

```bash
CONTROLLER_IMAGE=registry.example.com/edgion-controller:<tag>
GATEWAY_IMAGE=registry.example.com/edgion-gateway:<tag>
TEST_CLIENT_IMAGE=registry.example.com/edgion-test-client:<tag>
TEST_SERVER_IMAGE=registry.example.com/edgion-test-server:<tag>
ISOLATED_JOB_IMAGE=registry.example.com/edgion-isolated-test:<tag>
```

Validate conf does not contain Endpoint/EndpointSlice:

```bash
./edgion-tests/k8s/scripts/common/validate_no_endpoints.sh
```

Refresh k8s conf from `edgion-tests/integration/conf`:

```bash
```

Deploy integration environment (controller/gateway/test pods):

```bash
./edgion-tests/k8s/scripts/common/deploy_integration.sh --test-server-replicas 3
```

Run full Kubernetes integration (apply suites from `edgion-tests/k8s/conf` + run test_client):

```bash
./edgion-tests/k8s/scripts/run_k8s_integration.sh \
  --image-config /path/to/images.env \
  --full-test
```

`--cluster-mode auto` is the default and supports both local and remote
clusters. Use `--cluster-mode remote` only when auto-detection is unsuitable.

For an optional OrbStack/local-Docker workflow, build every image (including
the isolated-job image) and use the generated config:

```bash
./edgion-tests/k8s/scripts/orbstack/fast_build.sh --version local-dev
./edgion-tests/k8s/scripts/run_k8s_integration.sh \
  --local-image-mode local-existing \
  --image-config edgion-tests/k8s/scripts/config/images.env \
  --full-test
```

Two-phase behavior (default):

1. Prepare phase:
   - validate no Endpoint/EndpointSlice
   - generate runtime TLS/mTLS/backend CA secrets under `edgion-tests/k8s/generated/` (gitignored)
   - deploy workloads (optional)
   - strict apply all resources in `edgion-tests/k8s/conf` (any non-conflict error exits)
   - restart gateway and wait ready
2. Test phase:
   - run test suites via `test_client`

Run prepare only:

```bash
./edgion-tests/k8s/scripts/run_k8s_integration.sh --prepare-only
```

Run tests only on prepared cluster:

```bash
./edgion-tests/k8s/scripts/run_k8s_integration.sh --skip-prepare -r HTTPRoute -i Basic
```

Run partial test:

```bash
./edgion-tests/k8s/scripts/run_k8s_integration.sh -r HTTPRoute -i Match
./edgion-tests/k8s/scripts/run_k8s_integration.sh --start-from EdgionPlugins/JwtAuth
```

Run full mode and reload-two-round mode (aligned with old integration script):

```bash
./edgion-tests/k8s/scripts/run_k8s_integration.sh --full-test
./edgion-tests/k8s/scripts/run_k8s_integration.sh --with-reload
```

Run test_client directly:

```bash
./edgion-tests/k8s/scripts/common/run_test_client.sh -r EdgionPlugins -i JweDecrypt
```

Clean test environment:

```bash
./edgion-tests/k8s/scripts/cleanup.sh
./edgion-tests/k8s/scripts/cleanup.sh --with-images
```

Manual cleanup is not required before every run. A normal owned run starts
from a clean boundary and automatically removes its resources after collecting
diagnostics. A healthy harness-owned environment preserved with `--keep-alive`
can also be reused when its input state still matches. Run `cleanup.sh` when an
interrupted run leaves residue, the runner reports an ownership conflict, or
you explicitly want a fresh environment. The runner intentionally refuses to
pre-delete resources because the selected context may be a shared cluster.
CRDs are installation-scoped shared infrastructure and are never deleted by
the integration cleanup script.

## Notes

- Default deploy manifest path is `edgion-tests/k8s/kubernetes` (self-contained).
- Override with environment variable `K8S_DEPLOY_ROOT`.
- CRDs are applied from `config/crd` (not from `edgion-tests/k8s/kubernetes`).
- `--spec-profile` is kept for compatibility and is a no-op.
- `run_k8s_integration.sh` and `deploy_integration.sh` will fail fast if
  `edgion-tests/k8s/conf` contains `Endpoint` or `EndpointSlice`.
- `run_k8s_integration.sh` applies configs directly from `edgion-tests/k8s/conf`
  (no runtime conversion from `edgion-tests/integration/conf`).
- TLS-related Secrets are not committed as runtime outputs; they are generated
  before each prepare run into `edgion-tests/k8s/generated/` and ignored by git.
