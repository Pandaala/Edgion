# K8s Integration Test Entry

This directory is the Kubernetes test entry for Edgion examples.

## Layout

- `conf/`: Kubernetes-oriented test manifests converted from `examples/test/conf`.
- `scripts/`: wrappers to run Kubernetes integration in `edgion-deploy/kubernetes`.

## Usage

Run full Kubernetes integration:

```bash
./examples/k8stest/scripts/run_k8s_integration.sh
```

Deploy or clean test environment:

```bash
./examples/k8stest/scripts/deploy_integration.sh
./examples/k8stest/scripts/cleanup.sh
```

## Notes

- Default deploy repo path is `../edgion-deploy/kubernetes` relative to this repository.
- Override with environment variable `K8S_DEPLOY_ROOT`.
