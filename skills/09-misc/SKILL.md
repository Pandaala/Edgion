---
name: 09-misc
description: Use for pre-commit/local checks, CI/CD workflows, Docker builds, release automation, Center hot-replace deployment, or TLS troubleshooting.
---

# 09 Miscellaneous

> Edgion build and release workflow. Single-crate project; Rust toolchain is pinned by CI / `rust-toolchain.toml` (currently `1.92`, channel `stable`). Default features: `allocator-jemalloc` + `boringssl`.
> Four binaries: `edgion-gateway` (data plane), `edgion-controller` (control plane), `edgion-cli` (CLI), `edgion-center` (federation center).

## File list

| File | Topic | Status |
|------|-------|--------|
| [cicd/](cicd/) | CI/CD: local build, Docker build, GitHub Workflow, Release publishing | ✅ |
| [debugging-tls-gateway.md](debugging-tls-gateway.md) | Targeted TLS debugging in K8s environments (kubectl exec + log files + grep patterns) | ✅ |
| [debugging-conformance.md](debugging-conformance.md) | Conformance test debugging: rv/sv/access log triple, orbstack-dev.sh log commands, per-profile failure modes | ✅ |
| [03-dev-deploy-hot-replace.md](03-dev-deploy-hot-replace.md) | Dev deployment hot replace: native image + shell restart loop + cross compile + kubectl cp end-to-end flow | ✅ |

## Quick reference

### Local build
```bash
# Development mode (fast compile)
cargo build

# Release mode
cargo build --release

# Specify TLS backend (rustls experimental build)
cargo build --release --no-default-features --features "allocator-jemalloc,rustls"

# Code checks
cargo check && cargo clippy && cargo fmt --check
python3 cicd/checks/validate_agent_docs.py
```

Additional entry points:

- Local build details: [cicd/00-local-build.md](cicd/00-local-build.md)
- Feature combination details: [../02-features/01-binary-and-deployment/02-feature-flags.md](../02-features/01-binary-and-deployment/02-feature-flags.md)

### Docker build
```bash
# See the Dockerfiles under the cicd/docker/ directory
docker build -f cicd/docker/Dockerfile -t edgion .
```

### Release workflow
`build-image.yml` is currently **`workflow_dispatch`-only** (the tag-push trigger is commented out). Release is a manual dispatch:
```
write releases/v0.x.x.md → push tag v0.x.x → manually dispatch build-image.yml against that tag
  → Docker Hub pandaala/edgion-gateway, pandaala/edgion-controller → create-release
```
See [cicd/02-github-workflow.md](cicd/02-github-workflow.md) for the full job breakdown.

## Cargo features cheat sheet

| Feature | Description | Default |
|---------|-------------|---------|
| `allocator-jemalloc` | Use the jemalloc memory allocator | ✅ |
| `boringssl` | Use BoringSSL as the TLS backend | ✅ |
| `rustls` | Use rustls as the TLS backend | |
| `allocator-mimalloc` | Use the mimalloc memory allocator | |
| `allocator-system` | Use the system default allocator | |
| `legacy_route_tests` | Enable legacy route tests | |

> ⚠️ `boringssl` and `rustls` are mutually exclusive and cannot be enabled simultaneously.
