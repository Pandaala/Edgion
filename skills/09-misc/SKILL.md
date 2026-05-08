---
name: 09-misc
description: Miscellaneous skill for Edgion. Includes CI/CD (compiling binaries, feature flags, Docker builds, release workflows) and TLS debugging.
---

# 09 Miscellaneous

> Edgion build and release workflow. Rust 1.75+ single-crate project, default features: `allocator-jemalloc` + `boringssl`.
> Three binaries: `edgion-gateway` (data plane), `edgion-controller` (control plane), `edgion-cli` (CLI).

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
make check-agent-docs
```

Additional entry points:

- Local build details: [cicd/00-local-build.md](cicd/00-local-build.md)
- Feature combination details: [../02-features/01-binary-and-deployment/02-feature-flags.md](../02-features/01-binary-and-deployment/02-feature-flags.md)

### Docker build
```bash
# See the Dockerfiles under the docker/ directory
docker build -f docker/Dockerfile -t edgion .
```

### Release workflow
```
git tag v0.x.x → push → build-image.yml → Docker Hub pandaala/edgion-*
```

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
