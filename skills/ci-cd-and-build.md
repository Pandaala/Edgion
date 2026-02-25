---
name: ci-cd-and-build
description: Build, CI, and release pipeline reference.
---
# CI/CD and Build Guide

> Build, CI, and release pipeline reference.
>
> **TODO (2026-02-25): P1, New**
> - [ ] Cargo features explanation (allocator-jemalloc, boringssl, openssl, rustls, allocator-mimalloc, allocator-system, legacy_route_tests)
> - [ ] Local build commands (`cargo build --release`, feature combinations)
> - [ ] CI pipeline (`.github/workflows/ci.yml`: check, fmt, clippy, test; Rust 1.92)
> - [ ] Docker image build (cargo-chef multi-stage, multi-arch amd64/arm64)
> - [ ] Release workflow (git tag `v*` → `build-image.yml` → Docker Hub `pandaala/edgion-*`)
> - [ ] Common build issues (BoringSSL deps, linker issues, cross-compilation)
