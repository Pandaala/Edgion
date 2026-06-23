# edgion-tests — End-to-End Test Harness

This crate is the single home for Edgion's end-to-end tests. It is a Cargo
workspace member but is kept out of `default-members`, so a bare
`cargo build` / `cargo check --all-targets` does **not** compile it. Build it
explicitly with `cargo build -p edgion-tests`.

## Layout

```
edgion-tests/
├── Cargo.toml          # the test crate (lib + test_client/test_server/validator bins)
├── src/                # SHARED Rust harness: suites, test_client, test_server, validators
├── certs/              # SHARED generated certificates (gitignored); the runtime-contract
│                       #   path "edgion-tests/certs" that both in-process suites and the
│                       #   k8s test-client container mount expect
│
├── integration/        # ① in-process integration suite
│   ├── conf/           #   YAML fixtures by resource type (HTTPRoute, GRPCRoute, ...)
│   └── scripts/        #   run_integration.sh, prepare.sh, start_all.sh, gen_certs, ...
│
├── k8s/                # ② Kubernetes end-to-end suite
│   ├── conf/           #   K8s manifests (generated from integration/conf)
│   ├── kubernetes/     #   deployment manifests
│   └── scripts/        #   run_k8s_integration.sh, cleanup.sh, ...
│
└── conformance/        # ③ Gateway API conformance suite (standalone Go module)
    ├── go.mod / *.go
    ├── manifests/
    └── scripts/
```

The `src/` harness and the `certs/` directory are **shared** by the in-process
and Kubernetes suites — `test_client`/`test_server` are the same binaries in both,
and `edgion-tests/certs` is the path both honor (host repo path in-process, container
mount path in Kubernetes). Each suite then owns its own fixtures and runner scripts.

## ① In-process integration tests

```bash
# Full local integration sweep
./edgion-tests/integration/scripts/integration/run_integration.sh

# Targeted run (skip build)
./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r HTTPRoute -i Basic

# Build the harness binaries
cargo build -p edgion-tests --bins
```

## ② Kubernetes integration tests

See [k8s/README.md](k8s/README.md) for detailed instructions.

```bash
# Full Kubernetes integration test
./edgion-tests/k8s/scripts/run_k8s_integration.sh

# Run a specific suite / item
./edgion-tests/k8s/scripts/run_k8s_integration.sh -r HTTPRoute -i Match
```

## ③ Gateway API conformance tests

```bash
cd edgion-tests/conformance
go test -v -run TestConformance
```

## Related Documentation

- [User Guide](../docs/en/user-guide/README.md)
- [Ops Guide](../docs/en/ops-guide/README.md)
- [Developer Guide](../docs/en/dev-guide/README.md)
