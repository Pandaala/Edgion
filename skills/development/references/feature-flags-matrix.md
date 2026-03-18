# Feature Flag Matrix

Use this file when you need the concrete mapping from Cargo features to compile-time and runtime behavior.

## Source Of Truth

- `Cargo.toml`
- `src/lib.rs`
- `src/core/gateway/runtime/server/listener_builder.rs`
- `src/core/gateway/tls/runtime/`
- `src/bin/edgion_gateway.rs`
- `src/bin/edgion_controller.rs`
- `src/bin/edgion_ctl.rs`

## Declared Features

| Feature | Group | Default | Compile-Time Effect | Runtime Meaning |
|---------|-------|---------|---------------------|-----------------|
| `allocator-jemalloc` | allocator | yes | Pulls in `tikv-jemallocator` | Makes jemalloc the global allocator on non-MSVC targets via `src/lib.rs` |
| `allocator-mimalloc` | allocator | no | Pulls in `mimalloc` | Makes mimalloc the global allocator |
| `allocator-system` | allocator | no | No extra dep | Uses system allocator |
| `boringssl` | TLS backend | yes | Enables Pingora BoringSSL support and `boring-sys` | Unlocks current primary Gateway TLS runtime path |
| `openssl` | TLS backend | no | Enables Pingora OpenSSL support | Also unlocks Gateway TLS runtime path |
| `rustls` | TLS backend | no | Enables Pingora rustls backend | Does not by itself unlock all current Gateway TLS runtime code paths |
| `legacy_route_tests` | test/reserved | no | Declared only | I did not find active feature-gated code using it |

## Default Build

```bash
cargo build
```

Equivalent feature set:

```text
allocator-jemalloc + boringssl
```

## Mutual-Exclusion Intent

### Allocators

Intended one-of set:

- `allocator-jemalloc`
- `allocator-mimalloc`
- `allocator-system`

Recommended pattern:

```bash
cargo build --no-default-features --features "allocator-system,boringssl"
```

Do not stack multiple allocator features in the same build unless you are also changing the global allocator wiring in `src/lib.rs`.

### TLS Backends

Intended one-of set:

- `boringssl`
- `openssl`
- `rustls`

Recommended pattern:

```bash
cargo build --no-default-features --features "allocator-jemalloc,openssl"
```

## Important Runtime Caveat: `rustls` Is Not Full Gateway TLS Parity

Current Gateway TLS runtime code is still guarded in several places with:

```rust
#[cfg(any(feature = "boringssl", feature = "openssl"))]
```

Examples:

- `src/core/gateway/runtime/server/listener_builder.rs`
- `src/core/gateway/tls/runtime/gateway/mod.rs`
- `src/core/gateway/tls/runtime/backend/mod.rs`

Practical consequence:

- `rustls` is still used elsewhere in the repo for controller / CLI / client stacks.
- A `rustls` feature build is not the same thing as "all Gateway TLS listener and backend runtime features work".
- If the task involves HTTPS listeners, TLSRoute proxying, or data-plane TLS runtime behavior, prefer `boringssl` or `openssl` unless you are actively extending rustls support.

## Binary-Level Note

All three binaries currently install the rustls crypto provider at startup:

- `src/bin/edgion_gateway.rs`
- `src/bin/edgion_controller.rs`
- `src/bin/edgion_ctl.rs`

That startup step supports rustls-based dependencies in the repo. It does not override the Cargo feature matrix for Gateway data-plane TLS backends.

## Allocator Caveat

`src/lib.rs` gates jemalloc global-allocator installation with:

```rust
#[cfg(all(feature = "allocator-jemalloc", not(target_env = "msvc")))]
```

So if you are reasoning about Windows/MSVC, do not assume jemalloc is actually installed just because the feature is present.

## Example Combinations

| Goal | Command |
|------|---------|
| Default developer build | `cargo build` |
| Release with OpenSSL | `cargo build --release --no-default-features --features "allocator-jemalloc,openssl"` |
| Release with rustls experiment | `cargo build --release --no-default-features --features "allocator-jemalloc,rustls"` |
| Release with mimalloc + BoringSSL | `cargo build --release --no-default-features --features "allocator-mimalloc,boringssl"` |
| Release with system allocator + BoringSSL | `cargo build --release --no-default-features --features "allocator-system,boringssl"` |

## What To Say About `legacy_route_tests`

Current repo state:

- declared in `Cargo.toml`
- mentioned in docs/skill tables
- no active feature-gated call sites found by `rg`

Recommended documentation wording:

- "reserved / placeholder feature"
- "do not rely on it unless you also add real `cfg(feature = \"legacy_route_tests\")` code"
