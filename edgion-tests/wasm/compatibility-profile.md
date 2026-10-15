# Edgion Proxy-Wasm HTTP v1 fixture evidence

Profile: `edgion-proxy-wasm-http-v1`. The canonical function/signature inventory
is `edgion-wasm/src/lib.rs`; this document does not maintain another host registry.
The user contract is the [Wasm guide](../../docs/en/user-guide/http-route/filters/edgion-plugins/wasm.md).

## Selection and SDK decisions

| Fixture | SDK | Compiler / target | Decision |
|---|---|---|---|
| header-demo | Rust proxy-wasm =0.2.5 (Cargo.lock) | rustc 1.96.1 (31fca3adb), wasm32-unknown-unknown, release LTO | Baseline for header/local-response, callout and request-body behavior |
| lifecycle-demo | Rust proxy-wasm =0.2.5 (Cargo.lock) | Same Rust compiler/target | Baseline for object continuity, concurrent isolation, response EOS/trailers and cleanup |
| tinygo-policy | tetratelabs/proxy-wasm-go-sdk v0.24.0, tag commit 4b4272f742daa346fd1e904ef07410ac004649dd | TinyGo 0.38.0, Go 1.24.0, LLVM 19.1.2, Binaryen 123; wasi, scheduler=none | Builds; static analysis rejects six WASI imports; no runtime support claim |
| go-reactor-probe | proxy-wasm/proxy-wasm-go-sdk v0.0.0-20260105142703-44c7d5847745 | Go 1.24.0, GOOS=wasip1 GOARCH=wasm, buildmode=c-shared, trimpath | Builds; static analysis rejects WASI imports; no runtime support claim |
| C++ assessment (no fixture) | proxy-wasm-cpp-sdk 19c45f4df062e383a16bc3ac59c62f1948467866 | Upstream recommends Emscripten 3.1.67 | Not built or executed; defer until a concrete plugin justifies a maintained toolchain |

TinyGo's sample performs useful header policy (deny `x-policy-deny: 1` with a
fixed 403, otherwise set `x-policy-checked: yes`). It is an Edgion migration
sample, not evidence that a third-party plugin has migrated. The upstream Go
probe uses the same behavior so toolchain differences remain visible.

The Rust baseline covers these fixtures and the bounded behaviors below. It
is not certification of every plugin emitted by Rust SDK 0.2.5. A plugin can
still require unsupported calls, properties or scheduling semantics.

Primary sources inspected on 2026-10-15:

- [Go SDK pinned source](https://github.com/proxy-wasm/proxy-wasm-go-sdk/tree/44c7d58477456a40f475d515e29b32a006d5074b): Go 1.24+ reactor and additional host requirements.
- [TinyGo SDK v0.24.0](https://github.com/tetratelabs/proxy-wasm-go-sdk/tree/v0.24.0): separate TinyGo/WASI toolchain.
- [C++ build contract](https://github.com/proxy-wasm/proxy-wasm-cpp-sdk/blob/19c45f4df062e383a16bc3ac59c62f1948467866/docs/building.md): Emscripten 3.1.67, optional protobuf/RE2/Abseil build dependencies.
- [Proxy-Wasm specification](https://github.com/proxy-wasm/spec) and [SDK context model](https://github.com/proxy-wasm/proxy-wasm-cpp-sdk/blob/19c45f4df062e383a16bc3ac59c62f1948467866/docs/api_overview.md).

## Behavior matrix

| Surface | HTTP v1 contract / restriction | Evidence boundary |
|---|---|---|
| Imports and ABI | Exact canonical signatures, markers 0.2.0/0.2.1; unknown imports reject; standard unused trap stubs may load | Static analyzer tests, not instantiation proof |
| Properties | request.path/method/host, source.address, plugin_name=Wasm, response.code when provided; phase-local set_property | Host snapshots; no general Envoy property tree or persistent metadata |
| Destination names | Normalized origin URL plus explicit callout allowlist, canonical egress enforcement | Cluster names and Higress extensions are not translated |
| Request headers / local response | Header operations and fixed local responses; callout callback can resume or deny | Real Rust header guest tests |
| Request body | One complete terminal callback through canonical request BodyStorage; bounded rewrite | Existing body contract tests; not dynamic per-chunk Pause buffering |
| Response headers | Standalone synchronous hook or explicit attached companion | Attached companion retains request context; standalone does not |
| Response body | Current chunk, read-only; attached context can retain bounded incremental state | Real SDK lifecycle test; no full-response aggregate or backpressure via Pause |
| Trailers and EOS | Attached response trailers only; pre-trailer boundary is not EOS, headers-only EOS is terminal | Lifecycle test checks actual trailer callback and duplicate EOS suppression |
| Pause / resume | Request-header callout Pause, bounded callback rounds; continue_stream implements Request (0) only | Real callout test; no arbitrary async scheduling or response resume |
| Accepted no-ops | effective_context, done, continue_response return OK without semantics | Report warns; context switching/deferred completion/response resume remain unsupported |
| Lifetime | Request pins generation, exclusive instance and HTTP object; standalone response entries use per-hook contexts | Interleaved requests and generation retirement tested at runtime seam |
| Root / tick | Persistent dedicated timer VM; request root fields are not shared with it | Real SDK tick field test |
| Shared state | Explicit bounded host KV with CAS, private to configured runtime/generation; not cluster/persistent storage | Shared-data tests; shared counters alone do not prove HTTP continuity |
| Terminal cleanup | Normal terminal done/delete once; cancellation discards without guest re-entry; deferred done unsupported | Request/lifecycle tests; no promise of cleanup callbacks after cancellation |
| Unsupported operations | gRPC callouts, queues, foreign functions, guest metrics/log-level lookup trap if invoked | Existing trap/failure-policy test |

## WASI decision and priority gaps

On 2026-10-15 the user decided against adding WASI HTTP 0.3 support; reopening
requires an explicit new request. Component middleware, http-wasm's
`http_handler` ABI and core-Wasm SDK WASI imports are separate mechanisms.
The canonical analyzer rejects Components and the Gateway uses core `Module`
compilation. A future second ABI needs a concrete consumer and independent
continuation, capability, streaming/trailer, cancellation and resource-admission
design; runtime availability alone does not authorize integration.

The TinyGo probe requires `wasi_snapshot_preview1` imports `fd_write`,
`proc_exit`, `clock_time_get`, `args_sizes_get`, `args_get`, and `random_get`.
The Go reactor also imports `sched_yield`, environment functions, `poll_oneoff`,
`fd_close`, fd-stat functions and preopened-directory queries. The fixture
manifest records the complete observed distinct names. Both emit ABI 0.2.0;
that marker does not override their unsupported imports.

Do not strip imports, rewrite them to success stubs, or enable blanket WASI to
make these probes appear supported. A future minimal adapter must specify
reactor initialization (`_initialize`), exit/trap behavior, bounded diagnostic
writes, argument/environment policy, randomness/clocks, polling/cancellation
and deny-by-default filesystem/network/preopen capability. It must then prove
SDK object lifetime and request isolation with real execution. This work is
independent of WASI HTTP components and does not authorize them.

Priority: first maintain the Rust runtime evidence and exact report; next select
an actual TinyGo plugin and design the smallest necessary WASI adapter; assess
upstream Go scheduler/reactor cost separately; C++ requires a concrete consumer.
No market-share estimate is used to justify this ordering.

## Reproduction and evidence

Artifact hashes, toolchain strings and static results live in
[fixture-manifest.json](fixture-manifest.json). They identify exactly the checked
bytes; they are not execution certificates. Rust build command:

```bash
rustup run 1.96.1 rustc --version
./edgion-tests/wasm/build.sh
```

The build script explicitly selects Rust 1.96.1.
For the Go probes, install the versions in the table, then run:

```bash
./edgion-tests/wasm/build-compatibility.sh
cargo run -p edgion-cli --bin edgion-cli -- wasm inspect --file edgion-tests/wasm/artifacts/tinygo_policy.wasm --format json
```

The inspection intentionally exits nonzero for both Go probes. Tool paths may
be passed as `GO_BIN`, `TINYGO_BIN`, and `WASMOPT`. The script resolves paths
before changing directory, uses `GO_BIN` to select the Go SDK for both compilers,
and sets `GOTOOLCHAIN=local` to prevent automatic substitution. It replaces any
inherited `GOROOT` with the selected SDK and checks TinyGo uses that root.
`go.sum` pins module content;
building the fixtures requires fetching their pinned dependencies. Production
builds gain no Go, TinyGo, WASI or Emscripten dependency.

```bash
cargo test -p edgion-wasm --lib
cargo test -p edgion-cli --lib cli::commands::wasm
cargo test -p edgion-gateway --lib plugins::http::wasm -- --test-threads=1
```

Use serial runtime tests: concurrent independent tests can exhaust the existing
four-slot preparation budget. The first parallel run on 2026-10-15 failed with
`wasm preparation capacity exhausted`; this is retained as a verification
limitation, not silently counted as a pass.

Historical implementation verification on 2026-10-15: 27 analyzer tests and
263 serial Gateway Wasm tests passed; after terminal-safe text and toolchain
selection fixes, 27 analyzer, 15 CLI Wasm and three toolchain-script regressions
passed. Negative fixtures rebuilt reproducibly with conflicting inherited Go
settings. The follow-up changed no Gateway code and did not rerun its suite.
Independent plan/final reviews passed. Workspace check, format and applicable
guards passed; Clippy exited zero with unrelated existing warnings, rather than
a warning-free workspace. These are retained records, not a fresh cleanup-time
test run. Real SDK seam tests do not prove HTTP socket delivery, downstream
backpressure, complete Envoy/Higress conformance, or arbitrary SDK behavior.
