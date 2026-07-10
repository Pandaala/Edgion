# Reference proxy-wasm guest module

`header-demo` is the reference proxy-wasm guest used by Edgion's Wasm plugin
tests (`edgion-gateway/src/plugins/http/wasm/`) and by the `edgion-tests`
integration suite. It is built with the official
[`proxy-wasm` Rust SDK](https://crates.io/crates/proxy-wasm) (v0.2) so the
suites exercise a real SDK-produced module through the hand-written host ABI,
not just a hand-assembled WAT fixture.

## Contract

| Phase    | Condition                    | Behavior                                                    |
|----------|-------------------------------|---------------------------------------------------------------|
| request  | header `x-wasm-panic: 1`      | guest panics (host must recover; the gateway keeps serving)   |
| request  | header `x-wasm-deny: 1`       | local response `403` with body `wasm denied`                  |
| request  | otherwise                     | request header `x-wasm-added: yes` is added                   |
| response | always                        | response header `x-wasm-resp: yes` is added                   |

The guest source is `header-demo/src/lib.rs`.

## Rebuilding the artifact

The compiled module is committed at `artifacts/header_demo.wasm` on purpose,
so `edgion-tests` and the gateway's unit tests need no wasm toolchain to run.
Only rebuild it after changing `header-demo/src/lib.rs`.

Prerequisite:

```bash
rustup target add wasm32-unknown-unknown
```

Build:

```bash
./edgion-tests/wasm/build.sh
```

This compiles `header-demo` in release mode for `wasm32-unknown-unknown` and
copies the result to `artifacts/header_demo.wasm`.

## Standalone crate

`header-demo/Cargo.toml` declares its own `[workspace]` table, which keeps it
out of the main Edgion Cargo workspace (it only builds for wasm32 targets and
has no reason to participate in `cargo check --workspace` / `cargo test
--all`). Its `target/` build directory is git-ignored.
