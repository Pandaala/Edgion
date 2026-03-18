---
name: feature-flags-reference
description: Use when changing Cargo features, picking allocator or TLS backend combinations, or debugging build/runtime differences caused by feature flags.
---
# Feature Flags Reference

Use this skill when the task involves:

- changing `Cargo.toml` features
- choosing allocator or TLS backend combinations
- understanding why one build works and another does not
- documenting supported feature matrices for local build, Docker, or CI

## Read This First

- [references/feature-flags-matrix.md](references/feature-flags-matrix.md)

## High-Risk Rules

- Default features are `allocator-jemalloc` + `boringssl`.
- Allocator features are intended to be mutually exclusive:
  - `allocator-jemalloc`
  - `allocator-mimalloc`
  - `allocator-system`
- TLS backend features are intended to be mutually exclusive:
  - `boringssl`
  - `openssl`
  - `rustls`
- `rustls` as a Cargo feature does not currently mean full Gateway data-plane TLS parity. The Gateway TLS runtime and listener code still has multiple `#[cfg(any(feature = "boringssl", feature = "openssl"))]` gates.
- `legacy_route_tests` is currently declared in `Cargo.toml`, but I did not find active `#[cfg(feature = "legacy_route_tests")]` call sites in the repo. Treat it as reserved/placeholder until code uses it.

## Fast Commands

```bash
# Default build
cargo build

# Switch TLS backend
cargo build --no-default-features --features "allocator-jemalloc,openssl"
cargo build --no-default-features --features "allocator-jemalloc,rustls"

# Switch allocator
cargo build --no-default-features --features "allocator-mimalloc,boringssl"
cargo build --no-default-features --features "allocator-system,boringssl"
```

## Review Checklist

- Confirm the change keeps exactly one allocator feature enabled.
- Confirm the change keeps exactly one TLS backend feature enabled.
- If the build is for Gateway TLS termination or upstream TLS runtime paths, double-check whether the code path is gated on `boringssl` / `openssl`.
- If docs mention `legacy_route_tests`, label it as reserved unless you also add real feature-gated code.

## Related

- [../cicd/SKILL.md](../cicd/SKILL.md)
- [../cicd/00-local-build.md](../cicd/00-local-build.md)
