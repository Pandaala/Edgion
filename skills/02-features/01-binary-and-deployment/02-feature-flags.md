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
| `boringssl` | Yes | No | No | Default Gateway TLS backend; mutually exclusive with `rustls` |
| `rustls` | Yes | No | No | Alternate Gateway Pingora TLS backend |
| `fips` | Yes | No | No | Opt-in image build; implies `boringssl`, uses Boring 5's bundled FIPS module source by default |
| `legacy_route_tests` | Yes | No | No | Non-default legacy test surface |

Allocator selection and `#[global_allocator]` live in each binary entry point,
not in a workspace-root library. Only the Gateway selects a TLS backend.
Controller and CLI have allocator features only. There is no Pingora source
feature: the Gateway always builds against the Edgion fork family.

## Gateway TLS and FIPS

Gateway custom TLS termination paths are BoringSSL-only. A `rustls` build is not
feature-equivalent for HTTPS listeners, TLSRoute proxying, or all data-plane TLS hooks.
Use `boringssl` for the normal Gateway build.

The `fips` feature is a build-time property, not a runtime toggle. The ordinary
release workflow does not enable it. Use the dedicated Linux builder in
`cicd/build-image.sh`; Boring 5 builds its bundled FIPS module source by default
on the canonical amd64 path. The script rejects arm64 FIPS builds before
compilation because the upstream runtime integrity self-test fails there;
ordinary arm64 builds are unchanged. `BORING_BSSL_FIPS_PATH` is an upstream
prebuilt-tree escape hatch outside the canonical script contract. Runtime
`require_fips` and `fips_policy` are documented in
`../02-config/01-gateway-config.md`.

All three binaries still install the rustls crypto provider at startup for libraries
that use rustls; this does not change the Pingora data-plane backend.

The Controller no longer links Pingora. TLS-backend selection is Gateway-local.

## Pingora Fork Source Policy

The seven direct fork dependencies (`pingora-core`, `pingora-http`,
`pingora-proxy`, `pingora-limits`, `pingora-load-balancing`, `pingora-ketama`,
and `TinyUFO`) all track the fork's `edgion_v3` branch (`Pandaala/pingora`).
They must move as one source family; never pin or advance only one package.

Reproducibility comes from the committed `Cargo.lock`, which records the exact
Git commit for every fork package. CI, release workflows, and image builds use
`--locked`, so a moved branch cannot change a build until the lockfile is
intentionally refreshed and reviewed. Release milestones may replace the
branch selector with one protected immutable tag when a manifest-level release
identity is required; all seven entries must change together.

Run `python3 cicd/checks/check_pingora_source_policy.py` after any Pingora
dependency edit. It verifies the seven manifest entries and that the complete
fork family resolves to one commit.

## Build Examples

```bash
# Default members, default features
cargo build

# A specific default binary
cargo build -p edgion-gateway --bin edgion-gateway

# Gateway with mimalloc + BoringSSL
cargo build -p edgion-gateway --bin edgion-gateway --release \
  --no-default-features --features allocator-mimalloc,boringssl

# Controller with system allocator
cargo build -p edgion-controller --bin edgion-controller \
  --no-default-features --features allocator-system

# CLI with mimalloc (there is no CLI TLS-backend feature)
cargo build -p edgion-cli --bin edgion-cli \
  --no-default-features --features allocator-mimalloc
```

Never enable two allocators or both TLS backends in one package build. For the FIPS
image toolchain, follow `skills/09-misc/cicd/01-docker-build.md` rather than treating
the feature as a normal local build switch.
