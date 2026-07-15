---
name: feature-flags
description: Per-package Cargo feature reference for allocators, TLS backends, Gateway FIPS, and build commands.
---

# Feature Flags

Edgion has a virtual workspace manifest and no root package. Features belong to
individual binary crates, so feature-selecting commands must name a package with `-p`.

## Package Matrix

| Feature | Gateway | Controller | CLI | Default / rule |
|---------|---------|------------|-----|----------------|
| `allocator-jemalloc` | Yes | Yes | Yes | Default; mutually exclusive with the other allocators |
| `allocator-mimalloc` | Yes | Yes | Yes | Optional |
| `allocator-system` | Yes | Yes | Yes | Optional |
| `boringssl` | Yes | Yes | No | Default TLS backend where present; mutually exclusive with `rustls` |
| `rustls` | Yes | Yes | No | Alternate Pingora TLS backend |
| `fips` | Yes | No | No | Release/image-only; implies `boringssl`, requires the precompiled FIPS module |
| `legacy_route_tests` | Yes | No | No | Non-default legacy test surface |

Allocator selection and `#[global_allocator]` live in each binary entry point, not
in a workspace-root library. The Controller selects a Pingora TLS backend because it
links Pingora directly. The CLI has allocator features only.

## Gateway TLS and FIPS

Gateway custom TLS termination paths are BoringSSL-only. A `rustls` build is not
feature-equivalent for HTTPS listeners, TLSRoute proxying, or all data-plane TLS hooks.
Use `boringssl` for the normal Gateway build.

The `fips` feature is a build-time property, not a runtime toggle. It is enabled only
for release/CI image builds and requires
`BORING_BSSL_FIPS_PRECOMPILED_BCM_O`. Runtime `require_fips` and `fips_policy` are
documented in `../02-config/01-gateway-config.md`.

All three binaries still install the rustls crypto provider at startup for libraries
that use rustls; this does not change the selected Pingora data-plane backend.

## Build Examples

```bash
# Default members, default features
cargo build

# A specific default binary
cargo build -p edgion-gateway --bin edgion-gateway

# Gateway with mimalloc + BoringSSL
cargo build -p edgion-gateway --bin edgion-gateway --release \
  --no-default-features --features allocator-mimalloc,boringssl

# Controller with system allocator + rustls
cargo build -p edgion-controller --bin edgion-controller \
  --no-default-features --features allocator-system,rustls

# CLI with mimalloc (there is no CLI TLS-backend feature)
cargo build -p edgion-cli --bin edgion-cli \
  --no-default-features --features allocator-mimalloc
```

Never enable two allocators or both TLS backends in one package build. For the FIPS
image toolchain, follow `skills/09-misc/cicd/01-docker-build.md` rather than treating
the feature as a normal local build switch.
