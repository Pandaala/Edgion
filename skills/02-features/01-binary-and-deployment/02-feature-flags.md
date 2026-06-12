---
name: feature-flags
description: Full Cargo Feature Flags reference: allocator, TLS backend, mutual-exclusion rules, runtime notes, and build combinations.
---

# Feature Flags

## Source Locations

- `Cargo.toml`
- `src/lib.rs` (global allocator installation)
- `src/core/gateway/runtime/server/listener_builder.rs` (TLS listener guards)
- `src/core/gateway/tls/runtime/` (TLS runtime guards)
- `src/bin/edgion_gateway.rs`, `src/bin/edgion_controller.rs`, `edgion-cli/src/bin/edgion_ctl.rs` (rustls crypto provider initialization)

## Default Build

```bash
cargo build    # Equivalent to --features allocator-jemalloc,boringssl
```

## Feature Matrix

| Feature | Mutex Group | Default | Compile-time Effect | Runtime Meaning |
|---------|-------------|---------|---------------------|-----------------|
| `allocator-jemalloc` | Allocator | Yes | Pulls in `tikv-jemallocator` | Set as the global allocator on non-MSVC targets |
| `allocator-mimalloc` | Allocator | No | Pulls in `mimalloc` | Set as the global allocator |
| `allocator-system` | Allocator | No | No extra dependency | Use the system allocator |
| `boringssl` | TLS Backend | Yes | Enables Pingora BoringSSL + `boring-sys` | Unlocks the full Gateway TLS runtime path |
| `rustls` | TLS Backend | No | Enables the Pingora rustls backend | **Not fully equivalent**, see notes below |
| `legacy_route_tests` | Standalone | No | Declared only | Reserved/placeholder, currently no actual `cfg` call site |

## Mutual Exclusion Rules

- **Allocator group**: `jemalloc` / `mimalloc` / `system`, pick one. Do not stack multiple in the same build unless you also modify the global allocator wiring in `src/lib.rs`.
- **TLS Backend group**: `boringssl` / `rustls`, pick one.
- Cross-group combinations are free.

## rustls Notes

The Gateway TLS runtime code uses `#[cfg(feature = "boringssl")]` guards in several places:

- `src/core/gateway/runtime/server/listener_builder.rs`
- `src/core/gateway/tls/runtime/gateway/mod.rs`
- `src/core/gateway/tls/runtime/backend/mod.rs`

Practical impact:

- `rustls` is still used for the Controller / CLI / client-side TLS stack
- A `rustls` build ≠ "all Gateway TLS listener and backend runtime features are available"
- If your task involves HTTPS listeners, TLSRoute proxying, or the data-plane TLS runtime, use `boringssl`

## Allocator Notes

The global jemalloc installation guard in `src/lib.rs` is:

```rust
#[cfg(all(feature = "allocator-jemalloc", not(target_env = "msvc")))]
```

On Windows/MSVC, even if `allocator-jemalloc` is specified, it is not actually installed.

## Binary Startup Notes

All three binaries install the rustls crypto provider on startup:

- `src/bin/edgion_gateway.rs`
- `src/bin/edgion_controller.rs`
- `edgion-cli/src/bin/edgion_ctl.rs` (binary name `edgion-cli`)

This supports libraries in the repo that depend on rustls and **does not** override the Cargo feature matrix's choice of Gateway data-plane TLS backend.

## Build Examples

| Scenario | Command |
|----------|---------|
| Default development build | `cargo build` |
| Default release | `cargo build --release` |
| mimalloc + BoringSSL | `cargo build --release --no-default-features --features "allocator-mimalloc,boringssl"` |
| System allocator + BoringSSL | `cargo build --release --no-default-features --features "allocator-system,boringssl"` |
| rustls experiment | `cargo build --no-default-features --features "allocator-jemalloc,rustls"` |
