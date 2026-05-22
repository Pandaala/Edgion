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

- The default features are `allocator-jemalloc` + `boringssl`
- proto compilation comes from the repository root `build.rs`, so a working `protoc` is required locally
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

# Common examples
cargo build --release --example test_server --example test_client
```

## Feature combinations

Common combinations:

```bash
# Default: jemalloc + boringssl
cargo build

# rustls experimental build
cargo build --release --no-default-features --features "allocator-jemalloc,rustls"

# mimalloc + boringssl
cargo build --release --no-default-features --features "allocator-mimalloc,boringssl"

# system allocator + boringssl
cargo build --release --no-default-features --features "allocator-system,boringssl"
```

For more detailed feature semantics:

- [../../02-features/01-binary-and-deployment/02-feature-flags.md](../../02-features/01-binary-and-deployment/02-feature-flags.md)

## Where the build artifacts go

| Build mode | Artifact location |
|------------|-------------------|
| `cargo build` | `target/debug/` |
| `cargo build --release` | `target/release/` |
| `cargo build --target <triple> --release` | `target/<triple>/release/` |
| `cargo build --example <name> --release` | `target/release/examples/` |

Common files:

- `target/debug/edgion-gateway`
- `target/release/edgion-controller`
- `target/release/edgion-cli`
- `target/release/examples/test_server`

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
| Build succeeds but the Gateway TLS path is unavailable | Whether you mistakenly used the `rustls` combination; the Gateway TLS runtime currently still depends mainly on `boringssl` / `openssl` |

## When you need a clean, reproducible build environment

Look at:

- `docker/Dockerfile.builder`
- `docker/Dockerfile`
- `.github/workflows/build-image.yml`

These three are the project's actual ground truth for "the minimum set of build dependencies" and "how cross-compilation is done".
