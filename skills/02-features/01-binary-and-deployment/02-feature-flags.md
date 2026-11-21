---
name: feature-flags
description: Per-package Cargo feature reference for allocators, TLS backends, Gateway FIPS, optional Gateway subsystems, and build commands.
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
| `boringssl` | Yes | No | No | Default and only supported Gateway TLS backend; mutually exclusive with `rustls` |
| `rustls` | Dormant | No | No | Unsupported future-development seam; not a build, release, or runtime contract |
| `fips` | Yes | No | No | Opt-in image build; implies `boringssl`, uses Boring 5's bundled FIPS module source by default |
| `full` | Yes | No | No | Default; enables every optional Gateway subsystem below |
| `kafka` / `otlp` / `geoip` / `wasm` | Yes | No | No | Optional subsystems selected by `full`; additive |

Allocator selection and `#[global_allocator]` live in each binary entry point,
not in a workspace-root library. Only the Gateway selects a TLS backend.
Controller and CLI have allocator features only. There is no Pingora source
feature: the Gateway always builds against the Edgion fork family.
The allocator dependency crate names `tikv-jemallocator` and `mimalloc` are
implementation details, not selectable Cargo features of these binaries.

## Gateway TLS and FIPS

Gateway TLS support is BoringSSL-only. The dormant `rustls` feature does not
promise a clean Gateway build or any runtime capability, including plain paths,
HTTPS listeners, TLSRoute proxying, HeaderCertAuth portability, or upstream
`BackendTLSPolicy` parity. Do not select it for deployments, image builds, or CI.
Future support is tracked as one product task in
`tasks/block/rustls-gateway-tls-support.md`; isolated compatibility fixes are not
part of the current product scope.

The `fips` feature is a build-time property, not a runtime toggle. The ordinary
release workflow does not enable it. Use the dedicated Linux builder in
`cicd/build-image.sh`; Boring 5 builds its bundled FIPS module source by default
on amd64 and arm64. For explicit FIPS builds, Edgion pins the same BoringSSL
revision and applies upstream commit `4d6b2f971e456962b70e4e91768caffdb36fe016`
to fix arm64 integrity-hash relocation; ordinary builds do not use that source
override. `BORING_BSSL_FIPS_PATH` is an upstream prebuilt-tree escape hatch
outside the canonical script contract. Runtime
`require_fips` and `fips_policy` are documented in
`../02-config/01-gateway-config.md`.

All three binaries still install the rustls crypto provider at startup for libraries
that use rustls; this does not change the Pingora data-plane backend.

The Controller no longer links Pingora. TLS-backend selection is Gateway-local.

## Optional Gateway Subsystems

Each subsystem feature owns its third-party dependencies, so disabling it removes
that code and its CVE surface from the Gateway binary.

| Feature | Removes | Behavior when compiled out |
|---------|---------|----------------------------|
| `kafka` | `rskafka` | Kafka LinkSys never builds a client (runner logs the build failure); a primary Kafka log output fails Gateway startup; a Kafka fallback output is dropped with a WARN |
| `otlp` | `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp` | Same as `kafka` for OTLP LinkSys and OTLP log outputs; an OTLP SSL-log output also fails startup (the full build only warns that it is unsupported) |
| `geoip` | `maxminddb`, `memmap2` | No database can load; a configured `geoip.db_path` or `update_url` logs an error, lookups report `Unavailable`, and each `GeoIpLocation` plugin applies its own `failOpen` |
| `wasm` | `wasmtime`, `edgion-wasm` | Every `Wasm` entry fails preflight and is recorded as an invalid plugin (fail-closed); `wasm_limits` still parses and validates |

Disabled builds swap in an uninhabited placeholder for the provider client or
plugin, so the existing failure paths handle the configuration; there is no
extra request-path check. The Controller still validates these resources
without knowing which subsystems a Gateway binary contains, so it can report
`Accepted=True` for a plugin that the Gateway then rejects. Gateway capability
reporting is tracked in `tasks/todo/arch-03-build-time-extension-selection.md`.

The `edgion-waf` crate is not optional: `edgion-resources` validates WAF rules
with it and AI usage parsing reuses its bounded JSON reader. AI, Elasticsearch,
Redis, etcd, webhook and local-file outputs are always compiled. `LdapAuth` is
also always compiled: its client is the in-tree `edgion-ldap` leaf, which adds
no third-party dependency beyond tokio's I/O traits, `thiserror` and `zeroize`
(all already in the workspace), so a
feature gate would remove no CVE surface.

`edgion-gateway --print-build-capabilities` reports one
`subsystem-<name>=enabled|disabled` line per subsystem. The CI `gateway-core`
job checks and unit-tests the `core` combination (`--no-default-features
--features allocator-jemalloc,boringssl`) with `-D warnings`, and the
`gateway-build` job asserts that the default build reports every subsystem
enabled. `cicd/build-image.sh` warns when an explicit `FEATURES` list names no
subsystem; the FIPS recipe is `FEATURES="allocator-jemalloc,fips,full"`.

## Pingora Fork Source Policy

The seven direct fork dependencies (`pingora-core`, `pingora-http`,
`pingora-proxy`, `pingora-limits`, `pingora-load-balancing`, `pingora-ketama`,
and `TinyUFO`) all track the fork's `edgion_v0.9.1_main` branch (`Pandaala/pingora`).
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

# Gateway with mimalloc + BoringSSL and every optional subsystem
cargo build -p edgion-gateway --bin edgion-gateway --release \
  --no-default-features --features allocator-mimalloc,boringssl,full

# Minimal "core" Gateway: no Kafka, OTLP, GeoIP or Wasm
cargo build -p edgion-gateway --bin edgion-gateway --release \
  --no-default-features --features allocator-jemalloc,boringssl

# Core plus only OTLP log export
cargo build -p edgion-gateway --bin edgion-gateway --release \
  --no-default-features --features allocator-jemalloc,boringssl,otlp

# Controller with system allocator
cargo build -p edgion-controller --bin edgion-controller \
  --no-default-features --features allocator-system

# CLI with mimalloc (there is no CLI TLS-backend feature)
cargo build -p edgion-cli --bin edgion-cli \
  --no-default-features --features allocator-mimalloc
```

Never enable two allocators or both TLS backend features in one package build. Use
the supported `boringssl` backend for Gateway builds. For the FIPS
image toolchain, follow `skills/09-misc/cicd/01-docker-build.md` rather than treating
the feature as a normal local build switch.
