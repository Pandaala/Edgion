---
name: docker-build
description: Use when building Edgion images with Docker, understanding the multi-stage build, cargo-chef caching, feature passing, or runtime-image layout.
---
# Docker Build

Use this skill to answer:

- How the multi-stage build in `cicd/docker/Dockerfile` actually flows
- What layer `cargo-chef` caches
- What the build args `BINARY`, `BUILD_TYPE`, and `FEATURES` each affect
- When to use `cicd/docker/Dockerfile` vs. `cicd/docker/Dockerfile.runtime`

## Main files

- `cicd/docker/Dockerfile` — multi-stage source build (covered in detail below)
- `cicd/docker/Dockerfile.builder` — standalone build-environment image
- `cicd/docker/Dockerfile.runtime` — packages a pre-built artifact into a runtime image

Other variants in `cicd/docker/` (not detailed here):

- `cicd/docker/Dockerfile.controller-gateway` (+ `cicd/docker/controller-gateway-entrypoint.sh`) — packages pre-built binaries (controller + gateway + cli) into one standalone backend image (no dashboard/nginx). Driven by `build-image.sh --controller-gateway`

## Gateway images

`cicd/build-image.sh` builds the Gateway against the Edgion Pingora fork
(the only Pingora source):

| Gateway binary | Standard image | Controller-Gateway image |
|----------------|----------------|--------------------------|
| `edgion-gateway` | `edgion-gateway` | `edgion-controller-gateway` |

The Controller image is `edgion-controller` (the Controller does not link
Pingora).

All source-building image paths use Cargo's `--locked` mode, so images use the
exact Edgion Pingora commit recorded in `Cargo.lock`, even though the
workspace manifest follows the fork's `edgion_v3` branch.

```bash
# Default images
cicd/build-image.sh
```

## The 4 stages of `cicd/docker/Dockerfile`

### 1. `chef`

- Base image: `rust:<version>-slim`
- Installs `cargo-chef`
- Only responsible for providing the `cargo chef` command

### 2. `planner`

- Copies the workspace tree because cargo-chef must see every member manifest in the virtual workspace
- Runs `cargo chef prepare --recipe-path recipe.json`
- Generates the dependency recipe; does not compile business code

### 3. `builder`

- Installs build dependencies:
  - `g++`
  - `cmake`
  - `git`
  - `libclang-dev`
  - `perl`
  - `pkg-config`
  - `protobuf-compiler`
- Maps `PROFILE` to `--release` / no flag / `--profile <name>`, then runs
  package-qualified `cargo chef cook -p <package>` (feature args only for product crates)
- Then copies the full source and runs the actual `cargo build`

Caching meaning:

- When dependencies haven't changed, the `cargo chef cook` layer can be reused
- This avoids rebuilding the entire dependency tree every time business code changes

### 4. `runtime`

- Runtime image: `debian:trixie-slim`
- Installs `ca-certificates`, `libssl3`, and `procps`
- Creates a non-root user `edgion`
- Copies the final binary and default configuration

## Build arguments

| Argument | Default | Purpose |
|----------|---------|---------|
| `RUST_VERSION` | `1.96.1` | Rust toolchain version |
| `BINARY` | `edgion-gateway` | Name of the bin or example to build |
| `BUILD_TYPE` | `bin` | `bin` or `example` |
| `FEATURES` | `default` | Cargo features passed to `cargo build` / `cargo chef cook` |
| `PROFILE` | `release` | `dev`/`debug`, `release`, or a named profile such as `production` |

## Common commands

### Build a bin image

```bash
docker build \
  --build-arg BINARY=edgion-gateway \
  -t edgion/edgion-gateway:local \
  -f cicd/docker/Dockerfile .
```

### Build the controller

```bash
docker build \
  --build-arg BINARY=edgion-controller \
  -t edgion/edgion-controller:local \
  -f cicd/docker/Dockerfile .
```

### Build a test-harness binary

`BUILD_TYPE=example` is the build-arg token for the edgion-tests harness
binaries (`test_server` / `test_client`). They are no longer cargo examples —
they are `[[bin]]` targets of the `edgion-tests` crate, built internally with
`cargo build -p edgion-tests --bin <name>` (no `--features` flag, since the crate
defines none). The build-arg interface below is unchanged.

```bash
docker build \
  --build-arg BUILD_TYPE=example \
  --build-arg BINARY=test_server \
  -t edgion/edgion-test-server:local \
  -f cicd/docker/Dockerfile .
```

### Switch features

```bash
docker build \
  --build-arg BINARY=edgion-gateway \
  --build-arg FEATURES="allocator-jemalloc,rustls" \
  -t edgion/edgion-gateway:rustls \
  -f cicd/docker/Dockerfile .
```

### Build and verify a FIPS Gateway

Use the multi-architecture build script rather than a host-local Cargo build.
It supplies the Linux build environment Boring 5 needs and, whenever the exact
`fips` feature is selected, automatically runs the isolated FIPS module and TLS
profile test before packaging an image:

```bash
FEATURES="allocator-jemalloc,fips" \
  ./cicd/build-image.sh --arch amd64 --compile-only
```

The canonical source FIPS build currently supports amd64 only. The script
rejects `fips` with arm64 before compilation because the upstream BoringSSL
runtime integrity self-test fails for that source build. Ordinary non-FIPS
arm64 builds are unchanged. The ordinary release workflow and a default
invocation of `build-image.sh` stay non-FIPS. Boring 5 builds its bundled FIPS
module source by default. The upstream `BORING_BSSL_FIPS_PATH` prebuilt-tree
escape hatch is outside the canonical script contract.

The verification reuses
`edgion-tests/integration/scripts/integration/test_fips_policy.sh` in its real
module mode. It starts the built Gateway with `EDGION_REQUIRE_FIPS=1`, requires
`edgion_gateway_fips_module_enabled` to equal `1`, rejects a ChaCha20-only TLS
1.2 client, and accepts an approved AES-GCM client. It deliberately does not run
the full compatibility suite under the FIPS binary, so weak-cipher support tests
remain ordinary-BoringSSL coverage rather than becoming a growing skip list.

## When to use `cicd/docker/Dockerfile.runtime`

This file is not a source-build image; it "packages an already-built artifact into a runtime image".

Suitable for:

- CI has already cross-compiled the binary
- You only want to quickly produce a runtime image
- You already have `target/release` artifacts locally and don't want to rebuild inside Docker

Key arguments:

| Argument | Purpose |
|----------|---------|
| `BINARY` | `gateway` / `controller` or test-harness name |
| `BUILD_TYPE` | `bin` or `example` |
| `BINARY_PATH` | Pre-built artifact directory |
| `RUNTIME_BASE` | Runtime base image, defaults to `debian:bookworm-slim` |
| `EXTRA_PACKAGES` | Extra runtime apt packages |

Example:

```bash
docker build \
  -f cicd/docker/Dockerfile.runtime \
  --build-arg BINARY=gateway \
  --build-arg BUILD_TYPE=bin \
  --build-arg BINARY_PATH=target/release \
  -t edgion/edgion-gateway:runtime-local .
```

## Multi-architecture and the builder image

`cicd/docker/Dockerfile.builder` is a standalone "build environment image", more suitable for:

- Building Linux targets from any host
- Multi-architecture build experiments
- Avoiding embedding the full source-build process into the final business image

Example approach:

```bash
docker build --platform linux/arm64 -t edgion-builder -f cicd/docker/Dockerfile.builder .
docker run --rm --platform linux/arm64 -v "$(pwd)":/project edgion-builder \
  cargo build --release --target aarch64-unknown-linux-gnu
```

## Most common misconceptions

| Misconception | More accurate version |
|---------------|------------------------|
| `cargo-chef` caches the entire source build | It mainly caches the "dependency layer"; the final build after source changes still runs |
| `FEATURES` only affects the final `cargo build` | It also affects the `cargo chef cook` dependency-cache layer |
| `Dockerfile.runtime` can compile from source | It cannot; it depends on a pre-built artifact directory |
| Example builds also automatically include `edgion-cli` | When `BUILD_TYPE=example`, `edgion-cli` is not packaged |

## Related

- [00-local-build.md](00-local-build.md)
- [../../02-features/01-binary-and-deployment/02-feature-flags.md](../../02-features/01-binary-and-deployment/02-feature-flags.md)
- `.github/workflows/build-image.yml`
