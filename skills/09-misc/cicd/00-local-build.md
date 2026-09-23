---
name: local-build
description: Use when compiling Edgion locally, choosing a build mode, locating build artifacts, or debugging local build failures and feature combinations.
---
# Local Build

Use this skill to answer:

- Whether to run `cargo check`, `cargo build`, or `cargo build --release` locally right now
- Where binaries and examples are produced
- How to compile when switching the allocator / TLS backend
- Why local builds fail with missing dependencies, link errors, or cross-compilation failures

## Facts to check first

- Gateway default features are `allocator-jemalloc` + `boringssl`; the Gateway
  always builds against the Edgion Pingora fork
- The Edgion Pingora dependencies follow the `edgion_v5` branch, while the
  committed `Cargo.lock` pins the exact commit. Use `--locked` when reproducing
  CI or release builds.
- proto generation is owned by member build scripts (notably `edgion-common/build.rs`), so a working `protoc` is required locally
- The Docker build environment explicitly installs the following dependencies; treat it as a trustworthy reference for local dependencies:
  - `cmake`
  - `protobuf-compiler`
  - `pkg-config`
  - `libssl-dev`
  - `libclang-dev`
- For CI cross-compiling `aarch64-unknown-linux-gnu`, `gcc-aarch64-linux-gnu` / `g++-aarch64-linux-gnu` are also installed

## Most-used commands

```bash
# Fastest semantic check
cargo check

# Default development build
cargo build

# Release build
cargo build --release

# Common binaries
cargo build --release --bin edgion-gateway --bin edgion-controller --bin edgion-cli

# Common test-harness binaries (live in the edgion-tests crate)
cargo build --release -p edgion-tests --bin test_server --bin test_client
```

> The three main binaries are in `default-members`, so the bare `--bin <name>`
> form above resolves from the workspace root. The `edgion-tests` bins
> (`test_server`, `test_client`, `cache_residue`, …) are **not** in
> `default-members`, so they need an explicit `-p edgion-tests`.
>
> These `--release` commands are for **local** builds. Shipped images are built
> with the `production` profile (LTO + `codegen-units = 1`), which optimizes
> differently and lands in `target/<triple>/production/`. To reproduce the
> shipped artifact, follow
> [02-github-workflow.md → Reproduce the release binary builds](02-github-workflow.md#reproduce-the-release-binary-builds).
>
> Normal local development may omit `--locked`; Cargo can then refresh the
> lockfile when dependency changes are intentional. CI/release reproduction
> must add `--locked` and must not rewrite `Cargo.lock`. After advancing the
> Pingora fork, review that only its source-family entries changed and run
> `python3 cicd/checks/check_pingora_source_policy.py`.

## Feature combinations

Common combinations:

```bash
# Default: jemalloc + boringssl
cargo build

# mimalloc + boringssl
cargo build -p edgion-gateway --release --no-default-features --features "allocator-mimalloc,boringssl"

# system allocator + boringssl
cargo build -p edgion-gateway --release --no-default-features --features "allocator-system,boringssl"
```

The Gateway's rustls backend feature is an unsupported future-development seam
and is not a local-build target. See `tasks/pending/rustls-gateway-tls-support.md`.

For more detailed feature semantics:

- [../../02-features/01-binary-and-deployment/02-feature-flags.md](../../02-features/01-binary-and-deployment/02-feature-flags.md)

## Where the build artifacts go

| Build mode | Artifact location |
|------------|-------------------|
| `cargo build` | `target/debug/` |
| `cargo build --release` | `target/release/` |
| `cargo build --target <triple> --release` | `target/<triple>/release/` |
| `cargo build -p edgion-tests --bin <name> --release` | `target/release/` |

Common files:

- `target/debug/edgion-gateway`
- `target/release/edgion-controller`
- `target/release/edgion-cli`
- `target/release/test_server`

## Cross-compilation

What CI currently does for ARM64 Linux:

```bash
rustup target add aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu \
  --bin edgion-gateway --bin edgion-controller --bin edgion-cli
```

In Debian/Ubuntu-style environments you typically also need:

```bash
sudo apt-get install -y gcc-aarch64-linux-gnu g++-aarch64-linux-gnu
```

## Common failures and where to look first

| Symptom | Where to look first |
|---------|---------------------|
| `protoc` / protobuf-related errors | Whether `protobuf-compiler` is installed, and whether `build.rs` actually runs |
| BoringSSL build failure | Whether `cmake` is installed; whether `boringssl` is enabled in the current features |
| OpenSSL / `pkg-config` / `ssl` link failure | Whether `pkg-config`, `libssl-dev`, or the platform-equivalent OpenSSL development package is installed |
| ARM64 cross-compilation failure | Whether the `aarch64` cross compiler is installed and the target is set up correctly |
| Build succeeds but a custom Gateway TLS callback/path is unavailable | Check that the supported `boringssl` backend is enabled; the dormant rustls feature has no Gateway runtime contract |

## When you need a clean, reproducible build environment

Look at:

- `cicd/docker/Dockerfile.builder`
- `cicd/docker/Dockerfile`
- `.github/workflows/build-image.yml`

These three are the project's actual ground truth for "the minimum set of build dependencies" and "how cross-compilation is done".
