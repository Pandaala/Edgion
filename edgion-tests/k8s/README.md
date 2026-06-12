# K8s Integration Test Entry

This directory is the Kubernetes test entry for Edgion examples.

## Layout

- `conf/`: Kubernetes-oriented test manifests converted from `edgion-tests/integration/conf`.
- `scripts/`: K8s integration runner and helpers.
- `kubernetes/`: self-contained deployment manifests for controller/gateway/test workloads.

## Usage

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
./edgion-tests/k8s/scripts/run_k8s_integration.sh
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
./edgion-tests/k8s/scripts/cleanup.sh --with-crds --with-images
```

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
