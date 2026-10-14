# Reference proxy-wasm guest module

`header-demo` is the reference proxy-wasm guest used by Edgion's Wasm plugin
tests (`edgion-gateway/src/plugins/http/wasm/`) and by the `edgion-tests`
integration suite. It is built with the official
[`proxy-wasm` Rust SDK](https://crates.io/crates/proxy-wasm) (v0.2) so the
suites exercise a real SDK-produced module through the hand-written host ABI,
not just a hand-assembled WAT fixture.

## Contract

One guest covers every capability the host implements; behavior is selected by
request headers so unrelated scenarios never interfere.

| Phase / hook            | Condition                          | Behavior                                                                 |
|-------------------------|------------------------------------|--------------------------------------------------------------------------|
| request headers         | header `x-wasm-panic: 1`           | guest panics (host must recover; the gateway keeps serving)              |
| request headers         | header `x-wasm-deny: 1`            | local response `403` with body `wasm denied`                            |
| request headers         | header `x-wasm-callout-url: <o>`   | dispatch a GET `<o>/verify` callout forwarding `Authorization`, then Pause; callout `200` → resume (adds `x-wasm-added: yes`), else / failed → local `403` `callout denied`. `<o>` must be in `calloutAllowlist`. |
| request headers         | header `x-wasm-read-ticks: 1`      | read shared `ticks` counter into request header `x-wasm-ticks`          |
| request headers         | header `x-wasm-read-body: 1`       | read shared `bodybytes` counter into request header `x-wasm-bodybytes`  |
| request headers         | header `x-wasm-read-request-body: 1` | read the shared `requestbodybytes`, `requestbodycalls`, and `requestbodyeos` counters into the corresponding `x-wasm-*` request headers |
| request headers         | header `x-wasm-read-callout: 1`    | read shared `calloutticks` counter into request header `x-wasm-calloutticks` |
| request body            | header `x-wasm-rewrite-body: 1`    | exercise replace, insert, append, and delete; `abcdef` becomes `a+def!` |
| request body            | header `x-wasm-append-body: 1`     | append `created`, including to a bodyless request |
| request headers         | otherwise                          | request header `x-wasm-added: yes` is added                             |
| request body            | guest exports the hook and request headers continue | `on_http_request_body` runs once with the complete body and `end_of_stream = true`; for a non-empty body, add its length, callback count, and terminal count to shared `requestbodybytes`, `requestbodycalls`, and `requestbodyeos` |
| response headers        | always                             | response header `x-wasm-resp: yes` is added                             |
| response body           | wired at body-filter stage         | add each chunk's byte count to shared `bodybytes`; with `pluginConfig.string: response-body-probe`, return Pause for nonempty chunks so the host metric records the callback |
| background (`on_tick`)  | `on_configure` sets a 1s tick      | increment shared `ticks` each tick; when `pluginConfig` is a callout origin, also dispatch GET `<origin>/verify` (`authorization: Bearer valid-token`) — a 200 callback bumps shared `calloutticks` (origin must be in `calloutAllowlist`; a rejected dispatch is ignored) |

The guest source is `header-demo/src/lib.rs`. It uses `set_root_context` (for the
tick) with an HTTP context created via `create_http_context`.

The request-header hook runs before the request-body hook, so
`x-wasm-read-request-body: 1` reports counters accumulated by earlier requests.
The integration scenario sends a body first, then reads those counters on a
later request to prove the whole-body callback ran exactly once and received
the terminal flag.

Response-body filters use a separate Wasm runtime from request plugins, so a
request-header probe cannot read their shared counters. The BodyPlugins suite
instead checks a per-request increase in
`edgion_wasm_body_pause_unsupported_total` with the response-body probe enabled.
The host records Pause and continues streaming the response.

> **ABI export/import names.** The host and guest agree on the canonical
> proxy-wasm names the official SDK emits: the response-body hook is
> `proxy_on_response_body` (no `http` infix, same as the header hooks), and the
> callout host import is `proxy_http_call`. Getting these wrong is exactly the
> class of host↔SDK drift this reference guest exists to catch.

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

## Request lifecycle SDK fixture

`lifecycle-demo` pins the official Rust SDK at 0.2.5. Its HTTP object retains a
request marker and state through headers, complete request body, response
headers, streaming chunks, real trailers, and once-only completion. Its root
retains an incrementing field across ticks. The committed `lifecycle_demo.wasm`
is used by gateway unit tests to catch context replacement, interleaved request
contamination, duplicate EOS, and cancellation re-entry. Run `./build.sh` to
rebuild both fixtures; this standalone workspace is outside normal host tests.
