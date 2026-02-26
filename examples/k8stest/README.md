# K8s Integration Test Entry

This directory is the Kubernetes test entry for Edgion examples.

## Layout

- `conf/`: Kubernetes-oriented test manifests converted from `examples/test/conf`.
- `scripts/`: wrappers to run Kubernetes integration in `edgion-deploy/kubernetes`.

## Usage

Validate conf does not contain Endpoint/EndpointSlice:

```bash
./examples/k8stest/scripts/validate_no_endpoints.sh
```

Refresh k8s conf from `examples/test/conf`:

```bash
./examples/k8stest/scripts/refresh_conf_from_test.sh
```

Deploy integration environment (controller/gateway/test pods):

```bash
./examples/k8stest/scripts/deploy_integration.sh --spec-profile recommended --test-server-replicas 3
```

Run full Kubernetes integration (apply suites from `examples/k8stest/conf` + run test_client):

```bash
./examples/k8stest/scripts/run_k8s_integration.sh
```

Two-phase behavior (default):

1. Prepare phase:
   - validate no Endpoint/EndpointSlice
   - generate runtime TLS/mTLS/backend CA secrets under `examples/k8stest/generated/` (gitignored)
   - deploy workloads (optional)
   - strict apply all resources in `examples/k8stest/conf` (any non-conflict error exits)
   - restart gateway and wait ready
2. Test phase:
   - run test suites via `test_client`

Run prepare only:

```bash
./examples/k8stest/scripts/run_k8s_integration.sh --prepare-only
```

Run tests only on prepared cluster:

```bash
./examples/k8stest/scripts/run_k8s_integration.sh --skip-prepare -r HTTPRoute -i Basic
```

Run partial test:

```bash
./examples/k8stest/scripts/run_k8s_integration.sh -r HTTPRoute -i Match
./examples/k8stest/scripts/run_k8s_integration.sh --start-from EdgionPlugins/JwtAuth
```

Run full mode and reload-two-round mode (aligned with old integration script):

```bash
./examples/k8stest/scripts/run_k8s_integration.sh --full-test
./examples/k8stest/scripts/run_k8s_integration.sh --with-reload
```

Run test_client directly:

```bash
./examples/k8stest/scripts/run_test_client.sh -r EdgionPlugins -i JweDecrypt
```

Clean test environment:

```bash
./examples/k8stest/scripts/cleanup.sh
./examples/k8stest/scripts/cleanup.sh --with-images
./examples/k8stest/scripts/cleanup.sh --with-crds --with-images
```

## Notes

- Default deploy repo path is `../edgion-deploy/kubernetes` relative to this repository.
- Override with environment variable `K8S_DEPLOY_ROOT`.
- `run_k8s_integration.sh` and `deploy_integration.sh` will fail fast if
  `examples/k8stest/conf` contains `Endpoint` or `EndpointSlice`.
- `run_k8s_integration.sh` applies configs directly from `examples/k8stest/conf`
  (no runtime conversion from `examples/test/conf`).
- TLS-related Secrets are not committed as runtime outputs; they are generated
  before each prepare run into `examples/k8stest/generated/` and ignored by git.
