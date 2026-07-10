# Wasm Plugin

> **🔌 Edgion extension**
>
> Wasm executes a [proxy-wasm](https://github.com/proxy-wasm/spec) ABI guest module — an ordinary WebAssembly binary compiled from Rust, C++, Go, TinyGo, or any other proxy-wasm SDK — directly inside the gateway process via [wasmtime](https://wasmtime.dev/). It is provided by the `EdgionPlugins` CRD and is not part of standard Gateway API.

## What does Wasm solve?

proxy-wasm is a portable ABI for writing proxy extensions once and running them on any compliant host (Envoy, this gateway, or others) without recompiling the host itself. Instead of writing a native Rust plugin against Edgion's internal APIs, you compile a small WebAssembly module against the [proxy-wasm SDK](https://crates.io/crates/proxy-wasm) for your language of choice and hand the compiled `.wasm` bytecode to this plugin. The gateway compiles it once, caches the compiled module, and pools initialized instances so per-request execution is cheap.

This is a **v1, header-only** implementation: it exercises a deliberately small subset of the proxy-wasm ABI (see below), not the full spec.

## Supported ABI subset (v1)

### Hooks

| proxy-wasm hook | EdgionPlugins stage | When it runs |
|---|---|---|
| `proxy_on_context_create` / `proxy_on_vm_start` / `proxy_on_configure` | — (instance init) | Once per pooled instance, before any request is served |
| `proxy_on_request_headers` | `requestPlugins` (`RequestFilter`) | Request headers received, before the backend is contacted |
| `proxy_on_response_headers` | `upstreamResponseFilterPlugins` (`UpstreamResponseFilter`) | Upstream response headers received |
| `proxy_on_done` / `proxy_on_delete` | — (per-hook-call cleanup) | After each headers hook returns |

Wasm is **not** available at `upstreamResponseBodyFilterPlugins` or `upstreamResponsePlugins` (async) — see [Limitations](#v1-limitations).

### Host calls

| Host function | Supported | Notes |
|---|:---:|---|
| `proxy_get_header_map_pairs` / `proxy_get_header_map_value` | ✅ | Request and response header maps |
| `proxy_replace_header_map_value` / `proxy_add_header_map_value` / `proxy_remove_header_map_value` / `proxy_set_header_map_pairs` | ✅ | Set / add / remove / bulk-replace; applied to the real request/response after the hook returns |
| `proxy_send_local_response` | ✅ | Short-circuits the request with a status, body, and headers |
| `proxy_get_property` / `proxy_set_property` | ✅ (basic subset) | Readable keys: `request.path`, `request.method`, `request.host`, `source.address`, `plugin_name`, `response.code` (response phase only). `proxy_set_property` writes into the current phase's in-memory property map only (not persisted, not exposed elsewhere) |
| `proxy_get_buffer_bytes` | ✅ (VM/plugin configuration only) | Only `VmConfiguration` and `PluginConfiguration` buffer types are backed; other buffer types return not-found |
| `proxy_log` | ✅ | Forwarded to the gateway's own `tracing::debug!` output (target `wasm_guest`) — **never** written to the access log. The access log only records an aggregate count (see [PluginLog format](#pluginlog-format)) |
| `proxy_get_current_time_nanoseconds` | ✅ | Wall-clock time |
| `proxy_get_configuration` (legacy 0.1) | ✅ | Returns the same bytes as the `PluginConfiguration` buffer, for SDKs that still call the old accessor |
| `proxy_set_effective_context` / `proxy_set_tick_period_milliseconds` / `proxy_done` / `proxy_continue_stream` / `proxy_continue_request` / `proxy_continue_response` | Accepted no-ops | Return `STATUS_OK` without doing anything (tick period is accepted but never fires — there is no timer subsystem in v1) |
| Everything else (body hooks, HTTP/gRPC callouts, shared data, shared queues, foreign function calls, metrics, …) | ❌ | Not registered with the linker. If a guest module imports one of these and actually calls it, the call **traps** (the request then follows [failure semantics](#failure-semantics)); importing but never calling is harmless |

## Configuration reference

```yaml
type: Wasm
config:
  source:
    file: ./plugins/header_demo.wasm
  pluginConfig: '{"add_header":"x-wasm-added"}'
  failOpen: false
  timeoutMs: 100
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `source.file` | String | — | Local filesystem path to a `.wasm` file. Relative paths resolve against the gateway work directory. Exactly one of `source.file` / `source.inline` / `source.url` must be set. |
| `source.inline` | String (base64) | — | Wasm bytecode carried inline in the resource, base64-encoded. |
| `source.url` | String | — | An `http://` or `https://` URL the gateway fetches the bytecode from (fixed 10s fetch timeout, only a `2xx` response is accepted). **Requires `sha256`.** |
| `sha256` | String (64 hex chars) | — | SHA-256 of the raw bytecode, verified before compilation. Optional for `file` / `inline`; **required** for `url`. A mismatch is a load error — the module is never compiled. |
| `pluginConfig` | String | — | Opaque configuration string passed to the guest's `proxy_on_configure` hook, readable via the `PluginConfiguration` buffer (`proxy_get_buffer_bytes` / legacy `proxy_get_configuration`). |
| `vmConfig` | String | — | Opaque configuration string passed to the guest's `proxy_on_vm_start` hook, readable via the `VmConfiguration` buffer. |
| `failOpen` | Boolean | `false` | Governs behavior on load failure / trap / timeout / instance-acquire failure — see [Failure semantics](#failure-semantics). |
| `timeoutMs` | Integer | `100` | Per-hook wall-clock budget in milliseconds, enforced by wasmtime epoch interruption. Range `1..=10000`. |
| `instancePoolSize` | Integer | `8` | Max pooled instances kept per compiled module, per gateway process. Range `1..=64`. |

## The three source forms

### `file` — local filesystem

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: wasm-file
  namespace: edgion-default
spec:
  requestPlugins:
    - type: Wasm
      config:
        source:
          file: ./plugins/header_demo.wasm
        failOpen: false
        timeoutMs: 100
  upstreamResponseFilterPlugins:
    - type: Wasm
      config:
        source:
          file: ./plugins/header_demo.wasm
        failOpen: false
        timeoutMs: 100
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: wasm-route
  namespace: edgion-default
spec:
  parentRefs:
    - name: edgion-plugins-gateway
      namespace: edgion-test
  hostnames:
    - "wasm.example.com"
  rules:
    - matches:
        - path: { type: PathPrefix, value: "/" }
      filters:
        - type: ExtensionRef
          extensionRef: { group: edgion.io, kind: EdgionPlugins, name: wasm-file }
      backendRefs:
        - name: test-http
          namespace: edgion-test
          port: 30001
```

> The request phase and the response phase are **separate entries** (`requestPlugins` vs `upstreamResponseFilterPlugins`), same as other split-stage plugins. List only the phase(s) your guest module actually needs — omitting an entry costs nothing.

### `inline` — bytecode embedded in the resource

Useful for small guests you want to ship as part of the resource itself, with no external file or network dependency:

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: wasm-inline
  namespace: edgion-default
spec:
  requestPlugins:
    - type: Wasm
      config:
        source:
          inline: AGFzbQEAAAA...   # base64-encoded .wasm bytecode
        failOpen: true
        timeoutMs: 50
```

Two inline sources with different content never collide in the module cache — the cache key hashes the full base64 payload (SHA-256), not just its length.

### `url` — fetched over HTTP(S)

`sha256` is **mandatory** here so the gateway can verify the fetched bytes before compiling and running them:

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: wasm-url
  namespace: edgion-default
spec:
  requestPlugins:
    - type: Wasm
      config:
        source:
          url: "https://artifacts.example.com/plugins/header_demo.wasm"
        sha256: "3b1c...e4"   # 64 hex chars, required for url sources
        failOpen: false
        timeoutMs: 200
        instancePoolSize: 16
```

## Failure semantics

Four distinct failure modes all route through the same `failOpen` decision:

| Reason | Trigger | PluginLog |
|---|---|---|
| `load` | The module fails to compile, download, decode, or its `sha256` doesn't match. Load errors are cached for 30s so a broken source isn't retried on every request; after that window a fresh attempt is made. | `wasm load:fail-open` / `wasm load:fail-close` |
| `acquire` | The instance pool cannot produce a usable VM (e.g. `proxy_on_vm_start` / `proxy_on_configure` returned false during instantiation). | `wasm acquire:fail-open` / `wasm acquire:fail-close` |
| `trap` | The guest panics, hits `unreachable`, or otherwise faults while running a hook. | `wasm trap:fail-open` / `wasm trap:fail-close` |
| `not loaded` | Response-phase only: `UpstreamResponseFilter` is a synchronous hook (no `await`), so it uses a non-blocking cache lookup. If the module hasn't finished its (async) first load yet, this reason fires immediately and a background load is kicked off for next time. | `wasm not loaded:fail-open` / `wasm not loaded:fail-close` |

- **`failOpen: false`** (default) — the request is short-circuited with `500` and body `wasm plugin error`.
- **`failOpen: true`** — the failure is logged and the plugin chain continues as if Wasm were not configured.

### Trap poisoning and recovery

A trapped (or otherwise faulted) VM instance is **poisoned**: it is discarded rather than returned to the instance pool, freeing its slot. The next request that needs the module gets a fresh instance re-instantiated from the cached compiled module (no recompilation — only `Module::new` is expensive, `InstancePre::instantiate` is cheap). A guest panic never crashes, hangs, or otherwise impairs the gateway process — it only fails that one request (subject to `failOpen`).

### Timeout

Each hook call is bounded by `timeoutMs` via wasmtime **epoch interruption**: a background ticker increments the engine epoch every 10ms, and the deadline for a hook call is armed fresh at the start of *every* hook call (not once when the instance is leased) — a lease is typically held across the request-headers hook and the response-headers hook, with an upstream round trip in between, so a deadline set only at lease time would often already be stale by the time the response hook runs. A guest that blows its budget traps, which is handled identically to any other trap (poison + `failOpen` decision).

## Module cache lifecycle

The compiled-module cache (keyed by `source.cache_key()`) lives for the entire gateway process; be aware of what it does and does not do:

- **Never evicted on config change.** A compiled module stays cached and in memory for the life of the process. Editing the `EdgionPlugins` resource so `source.inline` or `source.url` carries different bytecode content creates a *new* cache entry (a different content hash) — it does not replace or evict the old one. The stale entry's memory is only reclaimed on gateway restart.
- **`file` sources are cached by path, not by content.** Overwriting the `.wasm` file on disk at the same path does **not** pick up the new build — the gateway never re-reads a `file` source once it has compiled it for that path. To roll out a new build, point `source.file` at a new path (e.g. version the filename) or restart the gateway. The same applies if you change only `sha256` while `source.url` is unchanged: the cache key is derived from the source, not from `sha256`, so an unchanged `url` with a new `sha256` reuses the old compiled module and never re-verifies against the new hash.
- **Multi-tenant sizing.** All Wasm modules across every `EdgionPlugins` resource share one engine-wide pooling allocator. When its instance slots are exhausted, `instantiate` fails (handled like any other `acquire` failure, subject to `failOpen`). Size `instancePoolSize` with the total instance count across all configured Wasm plugins in mind, not just one plugin in isolation.
- **Timeout floor.** Enforcement is epoch-based with a 10ms ticker, so effective granularity is roughly 10-20ms regardless of the configured `timeoutMs`. A `timeoutMs` set below 10 still allows the guest up to about one tick's worth of CPU time before it traps — it is not a hard sub-10ms bound.

## `PluginLog` format

The access log records one of:

- `wasm ok ops=N logs=M` — success; `N` header operations were applied to the request/response, `M` lines were emitted via `proxy_log` (their content goes to the debug log, not here).
- `wasm ok ops=N pause-ignored` — the guest returned `Action::Pause` without calling `proxy_send_local_response`; v1 cannot honor a bare pause (see [Limitations](#v1-limitations)), so the request/response continues normally.
- `wasm local <status>` — the guest called `proxy_send_local_response`; the gateway applied that status/headers/body and stopped.
- `wasm <reason>:fail-open` / `wasm <reason>:fail-close` — one of the four failure reasons above.

## v1 limitations

- **Headers only, no body hooks.** There is no `proxy_on_request_body` / `proxy_on_response_body` equivalent; a guest that only implements body hooks never gets invoked.
- **No HTTP or gRPC callouts.** `proxy_dispatch_http_call` and friends are not registered — calling them traps.
- **No timers.** `proxy_set_tick_period_milliseconds` is accepted (no-op) but no tick ever fires; there is no `proxy_on_tick`.
- **No shared data or shared queues.** The proxy-wasm shared-KV and shared-queue ABI is not implemented; calling it traps.
- **`Action::Pause` is not honored.** A hook that returns `Pause` without also calling `proxy_send_local_response` simply continues (logged as `pause-ignored`) — v1 has no mechanism to suspend a request mid-flight and resume it later.
- **No cross-phase guest state.** The request-headers hook and the response-headers hook of the *same* logical request may run on **different pooled VM instances** (each phase acquires its own lease from the pool), and each hook call gets a fresh proxy-wasm context id. Anything a guest stores in its own struct fields during `on_http_request_headers` is not guaranteed to be visible during `on_http_response_headers`. Use `pluginConfig` / `vmConfig` for static configuration, and rely on the gateway's own header pass-through (e.g. stash a value in a request header) if you need to correlate the two phases.

None of these are permanent architectural limits — they define what v1 exercises, not what proxy-wasm allows.

## Building a guest

Any language with a proxy-wasm SDK works, as long as the compiled module targets a linear-memory Wasm target (no WASI import required — `wasm32-unknown-unknown` is sufficient for the Rust SDK). The reference guest used by this repo's own tests is a good starting point:

- Source: `edgion-tests/wasm/header-demo/src/lib.rs`
- Built with the official [`proxy-wasm` Rust SDK](https://crates.io/crates/proxy-wasm) v0.2.
- `Cargo.toml`: `crate-type = ["cdylib"]`, `proxy-wasm = "0.2"`, and its own `[workspace]` table (kept out of the main Edgion Cargo workspace since it only builds for `wasm32-unknown-unknown`).
- Build: `rustup target add wasm32-unknown-unknown`, then `./edgion-tests/wasm/build.sh` — compiles in release mode and copies the artifact to `edgion-tests/wasm/artifacts/header_demo.wasm`.

Minimal shape of a guest (from the reference module):

```rust
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::Action;

proxy_wasm::main! {{
    proxy_wasm::set_http_context(|_context_id, _root_context_id| -> Box<dyn HttpContext> {
        Box::new(MyPlugin)
    });
}}

struct MyPlugin;
impl Context for MyPlugin {}

impl HttpContext for MyPlugin {
    fn on_http_request_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        self.set_http_request_header("x-wasm-added", Some("yes"));
        Action::Continue
    }

    fn on_http_response_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        self.set_http_response_header("x-wasm-resp", Some("yes"));
        Action::Continue
    }
}
```

See `edgion-tests/wasm/README.md` for the full contract the reference guest implements (including the panic and local-response test paths) and for how to rebuild the committed artifact after editing the guest source.

## FAQ

### Q: My guest module isn't taking effect on the response phase?

Confirm you listed the entry under `upstreamResponseFilterPlugins`, not `upstreamResponsePlugins` (that field belongs to a different, async stage used by other plugins such as ExtProc) — Wasm only supports the sync `UpstreamResponseFilter` stage on the response side.

### Q: The controller rejected my EdgionPlugins resource?

Common causes: `source` has zero or more than one of `file`/`inline`/`url` set; `source.url` is set without `sha256`; `sha256` isn't exactly 64 hex characters; `source.inline` isn't valid base64; `timeoutMs` is `0` or above `10000`; `instancePoolSize` is `0` or above `64`.

### Q: My module loads fine locally but fails in the gateway with a "load" fail-close?

Check the source form: `file` paths are resolved against the gateway's work directory (relative paths are not resolved against your local shell's cwd); `url` sources require the URL to respond within 10 seconds with a `2xx` status and bytes matching `sha256`.

### Q: Can the guest read a header the previous request-phase hook set?

Within the same hook call, yes — the guest's own writes are visible to subsequent `proxy_get_header_map_*` calls in that same call. Across the request-headers hook and the response-headers hook of the same request, no — see [No cross-phase guest state](#v1-limitations).

## Related links

- [ExtProc Plugin](./ext-proc.md) — the gRPC-based alternative to in-process Wasm execution
- [Filters overview](../README.md)
