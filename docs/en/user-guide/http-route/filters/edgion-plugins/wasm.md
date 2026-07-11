# Wasm Plugin

> **🔌 Edgion extension**
>
> Wasm executes a [proxy-wasm](https://github.com/proxy-wasm/spec) ABI guest module — an ordinary WebAssembly binary compiled from Rust, C++, Go, TinyGo, or any other proxy-wasm SDK — directly inside the gateway process via [wasmtime](https://wasmtime.dev/). It is provided by the `EdgionPlugins` CRD and is not part of standard Gateway API.

## What does Wasm solve?

proxy-wasm is a portable ABI for writing proxy extensions once and running them on any compliant host (Envoy, this gateway, or others) without recompiling the host itself. Instead of writing a native Rust plugin against Edgion's internal APIs, you compile a small WebAssembly module against the [proxy-wasm SDK](https://crates.io/crates/proxy-wasm) for your language of choice and hand the compiled `.wasm` bytecode to this plugin. The gateway compiles it once, caches the compiled module, and pools initialized instances so per-request execution is cheap.

The implemented ABI is a deliberate subset (see below): header read/write, local responses, **HTTP callouts with Pause/resume** (request phase), **read-only response-body observation**, **shared data**, and **timers** (`on_tick`). Body *modification* and request-body hooks are not implemented.

## Supported ABI subset

### Hooks

| proxy-wasm hook | EdgionPlugins stage | When it runs |
|---|---|---|
| `proxy_on_context_create` / `proxy_on_vm_start` / `proxy_on_configure` | — (instance init) | Once per pooled instance, before any request is served |
| `proxy_on_request_headers` | `requestPlugins` (`RequestFilter`) | Request headers received, before the backend is contacted |
| `proxy_on_http_call_response` | — (callout callback) | While the request is paused, once per completed callout dispatched from `proxy_on_request_headers` |
| `proxy_on_response_headers` | `upstreamResponseFilterPlugins` (`UpstreamResponseFilter`) | Upstream response headers received |
| `proxy_on_http_response_body` | `upstreamResponseBodyFilterPlugins` (`UpstreamResponseBodyFilter`) | Once per upstream body chunk, **read-only** (see [Body observation](#response-body-observation-read-only)) |
| `proxy_on_tick` | — (background) | Periodically, once the guest set a tick period (see [Timers](#shared-data-and-timers)) |
| `proxy_on_done` / `proxy_on_delete` | — (per-context cleanup) | After each per-request context finishes |

Wasm is **not** available at `upstreamResponsePlugins` (async response stage).

### Host calls

| Host function | Supported | Notes |
|---|:---:|---|
| `proxy_get_header_map_pairs` / `proxy_get_header_map_value` | ✅ | Request headers, response headers, and (inside the callout callback) `HttpCallResponseHeaders` — the callout's response headers plus a synthetic `:status` pair |
| `proxy_replace_header_map_value` / `proxy_add_header_map_value` / `proxy_remove_header_map_value` / `proxy_set_header_map_pairs` | ✅ | Set / add / remove / bulk-replace; applied to the real request/response after the hook returns. Header-map writes to the callout response map are rejected |
| `proxy_send_local_response` | ✅ | Short-circuits the request with a status, body, and headers. Also valid from inside `proxy_on_http_call_response` (deny after an auth callout) |
| `proxy_dispatch_http_call` | ✅ (request phase) | See [HTTP callouts](#http-callouts-pauseresume). In the response/body/tick phases it returns `UNIMPLEMENTED` (12) |
| `proxy_continue_request` / `proxy_continue_stream(Request)` | ✅ | Resumes a request paused on a callout. `proxy_continue_response` remains an accepted no-op |
| `proxy_get_shared_data` / `proxy_set_shared_data` | ✅ | Per-module KV with CAS — see [Shared data](#shared-data-and-timers) |
| `proxy_set_tick_period_milliseconds` | ✅ | Arms the per-module tick task; `0` stops it. Period floor 100ms |
| `proxy_get_property` / `proxy_set_property` | ✅ (basic subset) | Readable keys: `request.path`, `request.method`, `request.host`, `source.address`, `plugin_name`, `response.code` (response phase only). `proxy_set_property` writes into the current phase's in-memory property map only |
| `proxy_get_buffer_bytes` | ✅ | `VmConfiguration`, `PluginConfiguration`, `HttpCallResponseBody` (inside the callout callback), and `HttpResponseBody` (the **current chunk window** in the body hook) |
| `proxy_log` | ✅ | Forwarded to the gateway's own `tracing::debug!` output (target `wasm_guest`) — **never** written to the access log |
| `proxy_get_current_time_nanoseconds` | ✅ | Wall-clock time |
| `proxy_get_configuration` (legacy 0.1) | ✅ | Returns the same bytes as the `PluginConfiguration` buffer |
| `proxy_set_effective_context` / `proxy_done` | Accepted no-ops | Return `STATUS_OK` without doing anything |
| Everything else (request-body hooks, gRPC callouts, shared queues, foreign function calls, metrics, …) | ❌ | Not registered with the linker. If a guest module imports one of these and actually calls it, the call **traps** (the request then follows [failure semantics](#failure-semantics)); importing but never calling is harmless |

### ABI version check

At load time the host inspects the guest's `proxy_abi_version_*` marker export:

- `proxy_abi_version_0_2_0` / `proxy_abi_version_0_2_1` — accepted.
- Any other `proxy_abi_version_*` — **load error** with a clear "unsupported proxy-wasm ABI version" message (instead of a confusing runtime trap).
- No marker at all — accepted with a WARN (hand-written test modules and non-standard guests).

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
| `source.inline` | String (base64) | — | Wasm bytecode carried inline in the resource, base64-encoded. **Dev/test convenience only** — decoded size is capped at 1MiB (hard validation error above), and above 512KiB the gateway logs a WARN at load time. See [Choosing a source form](#choosing-a-source-form). |
| `source.url` | String | — | An `http://` or `https://` URL the gateway fetches the bytecode from. **Requires `sha256`.** Connection behavior is tunable via `source.fetch`. |
| `source.fetch` | Object | — | Optional fetch hardening for `url` sources (invalid with `file`/`inline`): TLS, auth, timeout, retry. See below. |
| `source.fetch.tls` | Object | — | The shared `TlsConfig` block: custom CA bundle (`validation.caCertificateRefs`) and/or client identity (`clientCertificateRef`) for mTLS to the artifact repository. Secret refs are resolved by the controller (cross-namespace refs need a `ReferenceGrant`). |
| `source.fetch.authHeaderSecretRef` | Object | — | `{name, namespace?, key}` of a Secret whose `key` (default `authorization`) holds the full `Authorization` header value sent with the fetch (e.g. `Bearer <token>`). If the ref is set but the Secret is not yet resolved, the load **fails** rather than silently fetching unauthenticated. |
| `source.fetch.timeoutMs` | Integer | `10000` | Per-attempt fetch timeout. Range `1..=60000`. |
| `source.fetch.retry` | Object | — | `{attempts, backoffMs}` — total attempts including the first (`1..=10`, default 3) and base backoff doubled per retry (default 500, max 30000). Fixed policy: transport errors and `5xx` retry; **`4xx` never retries**. |
| `sha256` | String (64 hex chars) | — | SHA-256 of the raw bytecode, verified before compilation. Optional for `file` / `inline`; **required** for `url`. |
| `pluginConfig` | String | — | Opaque configuration passed to `proxy_on_configure`, readable via the `PluginConfiguration` buffer. |
| `vmConfig` | String | — | Opaque configuration passed to `proxy_on_vm_start`, readable via the `VmConfiguration` buffer. |
| `failOpen` | Boolean | `false` | Governs behavior on load failure / trap / timeout / callout no-resume — see [Failure semantics](#failure-semantics). |
| `timeoutMs` | Integer | `100` | Per-hook wall-clock budget in milliseconds (epoch interruption). Range `1..=10000`. Body-chunk hooks run at `timeoutMs / 10`. |
| `instancePoolSize` | Integer | `8` | Max pooled instances kept per compiled module, per gateway process. Range `1..=64`. |
| `calloutAllowlist` | String list | `[]` | Origins (`scheme://host[:port]`) the guest may target with `proxy_dispatch_http_call`. **Empty = callouts disabled** (dispatch fails with a host error the guest can observe). Exact-origin match after normalization. |
| `calloutTimeoutMs` | Integer | `5000` | Upper bound on each callout's guest-supplied timeout (also the effective timeout when the guest passes 0). Range `1..=10000`. |

## Choosing a source form

| Form | Position | Size bound | Distribution path |
|---|---|---|---|
| `inline` | **Dev/test convenience.** Not a production distribution mechanism. | 1MiB decoded (hard cap); K8s object + etcd limits apply on top | Rides the CR through etcd; every conf_sync carries it in full |
| `file` | Modules baked into the gateway image or mounted via initContainer | none | Image build / volume mount |
| `url` | **Recommended for production** — internal artifact repository (Nexus, Artifactory, S3 presigned) | none | Gateway fetches directly, with `source.fetch` TLS/auth/retry and mandatory `sha256` |
| `oci://` | Roadmap (Istio-style OCI image distribution) — not yet implemented | — | — |

This mirrors Istio's posture: Istio's `WasmPlugin` has **no inline field** at all and treats `oci://` as the primary channel with `http(s)` as the second. Edgion keeps `inline` for fast iteration but caps and warns on it; a ConfigMap-based form was evaluated and rejected (same etcd object cap as inline, plus ref-tracking complexity, no added value).

### `url` with fetch hardening (production example)

```yaml
requestPlugins:
  - type: Wasm
    config:
      source:
        url: "https://artifacts.internal/wasm/header-demo-1.2.0.wasm"
        fetch:
          tls:
            enabled: true
            validation:
              caCertificateRefs:
                - { kind: Secret, name: artifact-repo-ca }
          authHeaderSecretRef:
            name: artifact-repo-token
            key: authorization        # Secret value e.g. "Bearer eyJ..."
          timeoutMs: 10000
          retry: { attempts: 3, backoffMs: 500 }
      sha256: "3b1c...e4"
```

Notes:

- Retries apply to transport errors and `5xx` only; a `401`/`403`/`404` fails immediately (an auth error will not heal by retrying, and retrying it floods the repository's logs).
- All retry attempts exhausting counts as ONE load failure, which is negative-cached for 30s like any other load error.
- The resolved `Authorization` value is redacted in the admin `/configclient` output.
- Distinct-URL fetches are capped at 4 in flight process-wide (startup-storm protection); same-URL callers already share one in-flight load.

## HTTP callouts (Pause/resume)

`proxy_dispatch_http_call` lets the guest call an external HTTP service mid-request — the proxy-wasm ecosystem's most common plugin shape (external auth, risk scoring, audit). Flow:

1. In `on_http_request_headers`, the guest dispatches one or more callouts and returns `Action::Pause`.
2. The gateway executes the callouts (asynchronously, off the guest's CPU budget) while the request is suspended.
3. Each result is delivered via `proxy_on_http_call_response`; the guest reads `HttpCallResponseHeaders` / `HttpCallResponseBody` and either resumes the request (`resume_http_request` / `proxy_continue_request`) or answers directly (`send_http_response` / `proxy_send_local_response`).

Semantics and guardrails:

- **Addressing**: the `upstream` parameter is the target **origin URL** (`https://auth.internal:8443`), not an Envoy cluster name. The `:path`, `:method`, and `:authority` pseudo headers from the dispatched header map drive the outbound request (`:authority` is sent as `Host`).
- **Allowlist**: the origin must exactly match an entry in `calloutAllowlist` (after normalization). No allowlist = no callouts. A denied dispatch returns `INTERNAL_FAILURE` from `proxy_dispatch_http_call` synchronously.
- **Request phase only**: the response-headers/body stages are synchronous and cannot await; dispatch there returns `UNIMPLEMENTED`. Ticks cannot dispatch either (yet).
- **Pause required**: `dispatch` + `Action::Continue` is not supported — the queued callouts are dropped and logged (`callouts-dropped=N`). Dispatch must be followed by `Pause`.
- **Budgets**: at most 8 dispatches per request, 4 callback rounds (a callback may dispatch follow-ups), 64 concurrent callouts process-wide, 256KiB response-body cap per callout. Each callout's timeout is `min(guest_timeout, calloutTimeoutMs)`.
- **Callout failure** (connect error, timeout, oversized body): the callback fires with **0 headers** — the proxy-wasm convention — and the guest decides its own fail behavior. The host never traps the guest for a failed callout.
- **Guest never resumes**: if after all callouts the guest neither continued nor answered, the host applies `failOpen` (reason `no-resume`).
- **Epoch budget**: `timeoutMs` bounds each *guest* hook invocation; the callout's network wait does not consume guest CPU budget.

### Callout vs ExtProc

Both integrate external decision services; they differ in direction:

- **ExtProc** (push): the gateway pushes every request through a resident external processor over gRPC. Choose it when the external service owns the logic and sees all traffic.
- **Wasm callout** (pull): guest logic lives in the gateway and calls out on demand. Choose it when the plugin decides *if/when* to consult an external service (e.g. cache-then-verify token checks).

## Response-body observation (read-only)

Listing a Wasm entry under `upstreamResponseBodyFilterPlugins` invokes `proxy_on_http_response_body` once per upstream chunk:

- `proxy_get_buffer_bytes(HttpResponseBody, start, max)` returns the **current chunk window only** — the gateway never buffers the body. Cross-chunk aggregation is the guest's job (its own memory or [shared data](#shared-data-and-timers)).
- **Strictly read-only**: the body cannot be modified, and the response cannot be short-circuited from the body phase (headers are already sent). A detection verdict can only be logged (`proxy_log`) or aggregated via shared data.
- `Action::Pause` from the body hook is meaningless in the read-only form and is ignored.
- Each chunk runs under a reduced CPU budget (`timeoutMs / 10`, floor 10ms) so a slow guest cannot stall a long stream; a trap in the body hook **never interrupts the stream** (the instance is discarded and streaming continues, regardless of `failOpen`).
- A guest that does not export `proxy_on_http_response_body` is a free no-op at this stage.

Request-body observation is **not implemented** (it requires the request-body buffering infrastructure, which the gateway does not have yet).

## Shared data and timers

### Shared data

`proxy_get_shared_data` / `proxy_set_shared_data` back a per-module KV store with proxy-wasm CAS semantics (per-key monotonically increasing CAS token; `set` with a nonzero expected CAS fails with `CasMismatch` on mismatch; CAS 0 writes unconditionally; a null value deletes the key).

- **Scope: one module (cache entry) within one gateway process.** All pooled instances of the module share it — across requests, phases, and the tick task. It is **not** cluster-level state: for cluster-wide coordination (e.g. global rate limiting) use `rate_limit_redis`, not shared data.
- Limits: 64KiB per value, 4MiB total per module. An oversized write returns `BAD_ARGUMENT`.
- Lifecycle: freed when the module's cache entry is evicted (see [Module cache lifecycle](#module-cache-lifecycle)); a reload that changes the module content starts with an empty store.

### Timers (`on_tick`)

A guest that calls `proxy_set_tick_period_milliseconds(N)` (typically from `on_configure`) gets `proxy_on_tick` invoked every `N` ms (floor 100ms) on a background task, one per module. The tick instance is leased from the same pool as request instances and runs under the normal `timeoutMs` epoch budget. The guest can change the period or stop ticking (`0`) from inside `on_tick`. The tick task stops automatically when the module's cache entry is evicted. Callouts from `on_tick` are not yet supported (dispatch returns `UNIMPLEMENTED`).

Typical pattern: `on_tick` refreshes a ruleset into shared data; request hooks read it.

## Failure semantics

All failure modes route through the same `failOpen` decision:

| Reason | Trigger | PluginLog |
|---|---|---|
| `load` | The module fails to download / decode / compile, its `sha256` doesn't match, its ABI version is unsupported, or instance init fails (`on_vm_start`/`on_configure` returned false). Load errors are cached for 30s. | `wasm load:fail-open` / `fail-close` |
| `acquire` | The instance pool cannot produce a usable VM at request time (e.g. pooling-allocator slots exhausted). | `wasm acquire:…` |
| `trap` | The guest panics, hits `unreachable`, blows its CPU budget, or otherwise faults while running a hook (including the callout callback and context close). | `wasm trap:…` |
| `callout` | Callout resolution failed structurally (e.g. the guest's callback trapped mid-delivery). | `wasm callout:…` |
| `no-resume` | The guest paused on callouts but neither resumed nor answered after all of them completed. | `wasm no-resume:…` |
| `not loaded` | Response-phase only: the sync hook uses a non-blocking cache lookup; if the module hasn't finished its (async) first load, this fires and a background load is kicked off. | `wasm not loaded:…` |

- **`failOpen: false`** (default) — the request is short-circuited with `500` and body `wasm plugin error`.
- **`failOpen: true`** — the failure is logged and the plugin chain continues.

### Trap poisoning and recovery

A trapped (or otherwise faulted) VM instance is **poisoned**: it is discarded rather than returned to the instance pool, freeing its slot. The next request gets a fresh instance re-instantiated from the cached compiled module (cheap — only `Module::new` is expensive). A guest panic never crashes, hangs, or otherwise impairs the gateway process.

### Timeout

Each hook call is bounded by `timeoutMs` via wasmtime **epoch interruption**: a background ticker increments the engine epoch every 10ms, and the deadline is re-armed at the start of *every* guest call (headers hooks, callout callbacks, body chunks, ticks). Effective granularity is ~10-20ms. Callout network waits happen *between* guest calls and never consume guest budget.

## Module cache lifecycle

The compiled-module cache is keyed by the source **plus, for `file` sources, the file's size+mtime**:

- **Refcount eviction.** Every live Wasm plugin instance pins its cache entry. When a reload/config change drops the last reference to an entry (e.g. inline content changed → new key), the orphaned entry is evicted after a **60s grace period** (which absorbs reload thrash). In-flight requests keep the entry alive via their own strong references; memory is freed when the last one finishes. Evictions show up in the `edgion_wasm_module_evicted_total` metric.
- **`file` sources are content-aware.** The runtime cache key includes the file's size and mtime, captured when the plugin is (re)built. Overwriting the `.wasm` at the same path **and triggering a plugin reload** (any change to the owning `EdgionPlugins` resource, or a full config resync) picks up the new content; the old entry is refcount-evicted. Without a reload the running plugin keeps its original key — a bare file overwrite alone still doesn't hot-swap.
- **Eager instance warm-up.** One instance is instantiated at load time, so guest init failures surface as clear load errors (negative-cached) instead of per-request acquire failures, and a tick period requested during `on_configure` starts ticking before the first request.
- **Multi-tenant sizing.** All Wasm modules share one engine-wide pooling allocator. Size `instancePoolSize` with the total across all configured Wasm plugins in mind.

## Metrics

All metrics carry a `module` label — the human-readable source descriptor (`file:<path>`, `url:<url>`, `inline:<sha8>`), bounded by the number of configured modules.

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `edgion_wasm_module_load_total` | counter | `result=ok\|error` | Load attempts (negative-cache hits not counted) |
| `edgion_wasm_module_compile_duration_ms` | histogram | | `Module::new` wall time |
| `edgion_wasm_module_evicted_total` | counter | | Refcount evictions |
| `edgion_wasm_instance_instantiate_total` | counter | | Fresh instantiations (pool misses) |
| `edgion_wasm_instance_discard_total` | counter | `reason=poisoned\|over_capacity` | Instances dropped instead of pooled |
| `edgion_wasm_pool_size` | gauge | | Current pooled instances |
| `edgion_wasm_hook_duration_ms` | histogram | `phase` | Guest hook wall time (`request_headers`, `response_headers`, `response_body`, `http_call_response`, `tick`) |
| `edgion_wasm_trap_total` | counter | `phase` | Traps / protocol violations |
| `edgion_wasm_fail_mode_total` | counter | `reason`, `mode=open\|close` | Fail-mode decisions |
| `edgion_wasm_callout_total` | counter | `result=ok\|error\|denied\|over_limit` | Callout outcomes |

A trap storm shows up as `edgion_wasm_trap_total` and `edgion_wasm_instance_discard_total{reason="poisoned"}` rising together.

## `PluginLog` format

- `wasm ok ops=N logs=M` — success; `N` header operations applied, `M` guest log lines (content goes to the debug log).
- `wasm ok ops=N pause-ignored` — the guest returned `Pause` without dispatching a callout or sending a local response; the request continues.
- `wasm callouts=N leftover=M` — request phase resolved `N` callouts (`M` dropped by the round cap).
- `wasm callouts-dropped=N` — the guest dispatched callouts but returned `Continue`; they were dropped (unsupported pattern).
- `wasm local <status>` — the guest sent a local response.
- `wasm <reason>:fail-open` / `wasm <reason>:fail-close` — one of the failure reasons above.

## Remaining limitations

- **No request-body hooks** (`proxy_on_request_body`): needs request-body buffering the gateway doesn't have; a guest that only implements request-body hooks never gets invoked.
- **Response body is observe-only**: no body rewrite, no short-circuit from the body phase.
- **No gRPC callouts** (`proxy_dispatch_grpc_call`): calling it traps.
- **Callouts are request-phase only**, and dispatch without `Pause` is dropped.
- **No shared queues, foreign functions, or guest-defined metrics**: calling them traps.
- **Per-request contexts are per-hook-call**: the request-headers and response-headers hooks of the same request may run on different pooled instances with fresh context ids. Correlate the phases through shared data or a stashed request header, not guest struct fields.
- **No OCI (`oci://`) source** yet — roadmap.

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

An external-auth callout guest looks like:

```rust
impl HttpContext for AuthPlugin {
    fn on_http_request_headers(&mut self, _n: usize, _eos: bool) -> Action {
        self.dispatch_http_call(
            "https://auth.internal:8443",       // origin — must be in calloutAllowlist
            vec![(":method", "GET"), (":path", "/verify"), (":authority", "auth.internal")],
            None, vec![], Duration::from_millis(500),
        ).unwrap();
        Action::Pause
    }
}

impl Context for AuthPlugin {
    fn on_http_call_response(&mut self, _token: u32, num_headers: usize, _body: usize, _trailers: usize) {
        if num_headers > 0 && self.get_http_call_response_header(":status").as_deref() == Some("200") {
            self.resume_http_request();
        } else {
            self.send_http_response(403, vec![], Some(b"denied"));
        }
    }
}
```

See `edgion-tests/wasm/README.md` for the full contract the reference guest implements and for how to rebuild the committed artifact after editing the guest source.

## FAQ

### Q: Which proxy-wasm ABI versions are supported?

`0.2.0` and `0.2.1` (the versions the Rust/C++ SDKs export as `proxy_abi_version_0_2_x`). A module exporting a different `proxy_abi_version_*` marker fails at load with an explicit message; a module exporting none loads with a WARN.

### Q: My callout is never executed?

Check, in order: (1) the target origin is listed in `calloutAllowlist` — empty list disables callouts entirely; (2) the guest returns `Action::Pause` after dispatching — dispatch followed by `Continue` is dropped (`callouts-dropped=N` in the access log); (3) the dispatch happens in the request phase — response/body/tick phases return `UNIMPLEMENTED`.

### Q: My guest module isn't taking effect on the response phase?

Confirm you listed the entry under `upstreamResponseFilterPlugins` (headers) or `upstreamResponseBodyFilterPlugins` (read-only body), not `upstreamResponsePlugins` (that async stage belongs to other plugins such as ExtProc).

### Q: The controller rejected my EdgionPlugins resource?

Common causes: `source` has zero or more than one of `file`/`inline`/`url` set; `source.url` without `sha256`; `sha256` isn't 64 hex chars; `source.inline` isn't valid base64 or decodes to more than 1MiB; `source.fetch` without `source.url`; `calloutAllowlist` entries that aren't clean `scheme://host[:port]` origins; `timeoutMs`/`calloutTimeoutMs` out of `1..=10000`; `instancePoolSize` out of `1..=64`.

### Q: I replaced the `.wasm` file on disk but the gateway still runs the old build?

The content-aware cache key is captured when the plugin is built. Touch the owning `EdgionPlugins` resource (any update) after overwriting the file — the reload re-stats the file, lands on a new cache entry, and the old one is evicted after the grace period.

### Q: Can two requests share state?

Yes — via `proxy_{get,set}_shared_data`, which is shared across all instances of the same module **within one gateway process**. It is not replicated across gateway replicas; cluster-level coordination still belongs to `rate_limit_redis`.

## Related links

- [ExtProc Plugin](./ext-proc.md) — the gRPC push-model alternative (see [Callout vs ExtProc](#callout-vs-extproc))
- [Filters overview](../README.md)
