# Edgion Kubernetes & Conformance Suites

This directory holds the two **system-level** test suites that are independent of
the in-process integration harness:

```
examples/
├── gateway-api-conformance/   # Gateway API conformance tests (standalone Go module)
│   ├── conformance_test.go
│   └── manifests/
└── k8stest/                   # Kubernetes end-to-end tests
    ├── conf/                  # K8s test manifests (derived from edgion-tests/conf)
    ├── kubernetes/            # Deployment manifests
    └── scripts/               # K8s test runner scripts
```

> **The local integration harness moved.** The Rust test code, orchestration
> scripts, YAML fixtures, and certificates now live in the **`edgion-tests`**
> workspace crate, not here:
>
> ```
> edgion-tests/
> ├── src/        # Rust: suites, test_server, validators (test_client, etc.)
> ├── scripts/    # run_integration.sh, prepare.sh, start_all.sh, gen_certs, ...
> ├── conf/       # YAML fixtures by resource type (HTTPRoute, GRPCRoute, ...)
> └── certs/      # auto-generated test certificates (gitignored)
> ```

## Local integration tests (now under `edgion-tests/`)

```bash
# Full local integration sweep
./edgion-tests/scripts/integration/run_integration.sh

# Targeted run (skip build)
./edgion-tests/scripts/integration/run_integration.sh --no-prepare -r HTTPRoute -i Basic

# Build the harness binaries
cargo build -p edgion-tests --bins
```

## Kubernetes integration tests

See [k8stest/README.md](k8stest/README.md) for detailed instructions.

```bash
# Full Kubernetes integration test
./examples/k8stest/scripts/run_k8s_integration.sh

# Run a specific suite / item
./examples/k8stest/scripts/run_k8s_integration.sh -r HTTPRoute -i Match
```

## Gateway API conformance tests

```bash
cd examples/gateway-api-conformance
go test -v -run TestConformance
```

## Related Documentation

- [User Guide](../docs/en/user-guide/README.md)
- [Ops Guide](../docs/en/ops-guide/README.md)
- [Developer Guide](../docs/en/dev-guide/README.md)
