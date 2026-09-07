# Wasm Plugin

> Migrating from Envoy? `edgion-cli wasm inspect` checks a module against
> Edgion's exact host profile, and `edgion-cli wasm convert-envoy` strictly
> converts one v3 HTTP Wasm filter or bare PluginConfig into a `WasmConfig`
> fragment. See the [CLI guide](../../../../ops-guide/edgion-cli.md#wasm---local-inspection-and-envoy-migration).
> This is a migration assistant, not seamless-compatibility proof. You must
> choose the EdgionPlugins stage, and response-body processing remains
> per-chunk streaming. Edgion also fixes the guest-visible `plugin_name` to
> `Wasm`; the converter warns even when Envoy's plugin name is empty.

> **🔌 Edgion extension**
>
> Wasm executes a [proxy-wasm](https://github.com/proxy-wasm/spec) ABI guest module — an ordinary WebAssembly binary compiled from Rust, C++, Go, TinyGo, or any other proxy-wasm SDK — directly inside the gateway process via [wasmtime](https://wasmtime.dev/). It is provided by the `EdgionPlugins` CRD and is not part of standard Gateway API.

## What does Wasm solve?

proxy-wasm is a portable ABI for writing proxy extensions once and running them on any compliant host (Envoy, this gateway, or others) without recompiling the host itself. Instead of writing a native Rust plugin against Edgion's internal APIs, you compile a small WebAssembly module against the [proxy-wasm SDK](https://crates.io/crates/proxy-wasm) for your language of choice and hand the compiled `.wasm` bytecode to this plugin. The gateway compiles it once, caches the compiled module, and pools initialized instances so per-request execution is cheap.

The implemented ABI is a deliberate subset (see below): header read/write, local responses, **HTTP callouts** (request phase with Pause/resume, and from `on_tick`), **request-body buffer mode** (whole-body, guest-declared, can deny or rewrite), **read-only response-body observation** (per-chunk), **shared data**, and **timers** (`on_tick`). Whole-response buffering and response-body modification are not supported: the main upstream response always remains streaming.

## Supported ABI subset

### Hooks

| proxy-wasm hook | EdgionPlugins stage | When it runs |
|---|---|---|
| `proxy_on_context_create` / `proxy_on_vm_start` / `proxy_on_configure` | — (instance init) | Once per pooled instance, before any request is served |
| `proxy_on_request_headers` | `requestPlugins` (`RequestFilter`) | Request headers received, before the backend is contacted |
| `proxy_on_request_body` | `requestPlugins` (`RequestFilter`), same entry as headers | Once per request, **after** `proxy_on_request_headers`, only if the guest module exports this hook — see [Request-body buffer mode](#request-body-buffer-mode) |
| `proxy_on_http_call_response` | — (callout callback) | Once per completed callout: on the paused request context for callouts dispatched from `proxy_on_request_headers`, or on the root context for callouts dispatched from `proxy_on_tick` |
| `proxy_on_response_headers` | `upstreamResponseFilterPlugins` (`UpstreamResponseFilter`) | Upstream response headers received |
| `proxy_on_response_body` | `upstreamResponseBodyFilterPlugins` (`UpstreamResponseBodyFilter`) | Once per upstream body chunk, **read-only** (see [Response-body observation](#response-body-observation-read-only)) |
| `proxy_on_tick` | — (background) | Periodically, once the guest set a tick period (see [Timers](#shared-data-and-timers)) |
| `proxy_on_done` / `proxy_on_delete` | — (per-context cleanup) | After each per-request context finishes |

Wasm is **not** available at `upstreamResponsePlugins` (async response stage).
If a module needs the complete request body, place it in `requestPlugins`.
`RequestBodyBuffer` can preserve a request body for supported later consumers
such as DSL, but response-stage Wasm host phases do not expose that cache.

### Host calls

| Host function | Supported | Notes |
|---|:---:|---|
| `proxy_get_header_map_pairs` / `proxy_get_header_map_value` | ✅ | Request headers, response headers, and (inside the callout callback) `HttpCallResponseHeaders` — the callout's response headers plus a synthetic `:status` pair |
| `proxy_replace_header_map_value` / `proxy_add_header_map_value` / `proxy_remove_header_map_value` / `proxy_set_header_map_pairs` | ✅ | Set / add / remove / bulk-replace; applied to the real request/response after the hook returns. Header-map writes to the callout response map are rejected |
| `proxy_send_local_response` | ✅ | Short-circuits the request with a status, body, and headers. The body is forwarded byte-for-byte, so a binary payload (an image, protobuf, ...) is delivered unchanged; an empty body produces a bodyless response. Also valid from inside `proxy_on_http_call_response` (deny after an auth callout) |
| `proxy_http_call` | ✅ (request-headers + tick phases) | See [HTTP callouts](#http-callouts-pauseresume). In the request-body, response-headers, and response-body phases it returns `UNIMPLEMENTED` (12); in the tick phase it requires an unambiguous allowlist (see [Timers](#shared-data-and-timers)) |
| `proxy_continue_request` / `proxy_continue_stream(Request)` | ✅ | Resumes a request paused on a callout. `proxy_continue_response` remains an accepted no-op |
| `proxy_get_shared_data` / `proxy_set_shared_data` | ✅ | Per-module KV with CAS — see [Shared data](#shared-data-and-timers) |
| `proxy_set_tick_period_milliseconds` | ✅ | Arms the per-module tick task; `0` stops it. Period floor 100ms |
| `proxy_get_property` / `proxy_set_property` | ✅ (basic subset) | Readable keys: `request.path`, `request.method`, `request.host`, `source.address`, `plugin_name` (fixed to `Wasm`), `response.code` (response phase only). `proxy_set_property` writes into the current phase's in-memory property map only |
| `proxy_get_buffer_bytes` | ✅ | `VmConfiguration`, `PluginConfiguration`, `HttpCallResponseBody` (inside the callout callback), `HttpRequestBody` (the **whole buffered request body** in the request-body hook — see [Request-body buffer mode](#request-body-buffer-mode)), and `HttpResponseBody` (the **current chunk window** in the response-body hook) |
| `proxy_set_buffer_bytes` | ✅ (request body only) | Replaces, inserts, appends, or deletes bytes in `HttpRequestBody` during the whole request-body hook. Response-body writes return `STATUS_UNIMPLEMENTED` (12) |
| `proxy_log` | ✅ | Guest text is retained only within the current hook's bounded in-memory diagnostics and then discarded; the access log records only the bounded line count, never the guest-controlled text |
| `proxy_get_current_time_nanoseconds` | ✅ | Wall-clock time |
| `proxy_get_configuration` (legacy 0.1) | ✅ | Returns the same bytes as the `PluginConfiguration` buffer |
| `proxy_set_effective_context` / `proxy_done` | Accepted no-ops | Return `STATUS_OK` without doing anything |
| Standard SDK-declared but unimplemented calls (gRPC callouts, shared queues, foreign function calls, guest metrics, and log-level lookup) | ⚠️ | Their exact standard signatures are accepted because SDKs may declare them even when unused. The compatibility report marks them `known_unsupported_trap` and warns; invoking one traps and follows [failure semantics](#failure-semantics) |
| Everything else | ❌ | Rejected by the compatibility check at module load. Truly unknown imports are not accepted even when the guest never calls them |

### ABI version check

After source integrity verification and before Wasmtime compilation, the host parses the core Wasm module and checks its ABI markers, imports, and recognized hook exports:

- `proxy_abi_version_0_2_0` / `proxy_abi_version_0_2_1` — accepted.
- Any unsupported marker, or any module exporting more than one ABI marker, is a **load error**.
- No marker at all — accepted with a WARN (hand-written test modules and non-standard guests).
- Every import must be a function from module `env`, appear in this host's exact profile, and exactly match that function's parameter and result types.
- Standard Proxy-Wasm hook names must be function exports with their exact SDK ABI signatures. Any other `proxy_on_*` export is rejected as an unknown or misspelled hook; unrelated custom exports remain allowed. WebAssembly components, malformed modules, non-function imports, wrong import modules, unknown imports, and signature mismatches are rejected before JIT compilation.
- The 64MiB source-byte cap is separate from structural complexity limits. Before full validation, the analyzer rejects modules exceeding 100,000 combined entries across validator-indexed core sections (types, imports, functions, tables, memories, tags, globals, exports, elements, and data), 50,000 combined import/export inventory items, 4MiB of import/export name bytes, 100,000 function-type signature cells, or 100,000 signature cells materialized in the report. Expanded locals are capped at 100,000 per function and 1,000,000 per module; each encoded function body is capped at 256KiB to bound per-function validator work. It returns `analysis_limit_exceeded`; it never truncates the report and then claims compatibility.

Compatibility failures intentionally use one bounded generic load error. The gateway does not copy attacker-controlled import/export names or parser detail into request-visible failure text.

In compatibility reports, `compatible: true` means **load-compatible with this host profile**, not that every declared feature is implemented or every exported hook is invoked. Check the `support` field on imports and hooks, plus the warnings list. `known_unsupported_trap` imports are safe to leave unused but trap if called. Standard SDK hooks marked `known_not_invoked`—including request/response trailers, queue-ready, gRPC callbacks, foreign-function callbacks, and stream callbacks—are accepted because SDKs commonly export them by default, but Edgion never calls them. Hooks marked `implemented` do not produce that warning.

## Configuration reference

```yaml
type: Wasm
config:
  source:
    file: ./plugins/header_demo.wasm
  pluginConfig:
    json:
      add_header: x-wasm-added
  failOpen: false
  timeoutMs: 100
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `source.file` | String | — | Local filesystem path to a `.wasm` file. Relative paths resolve against the gateway work directory. Exactly one of `source.file` / `source.inline` / `source.url` / `source.oci` must be set. |
| `source.inline` | String (base64) | — | Wasm bytecode carried inline in the resource, base64-encoded. **Dev/test convenience only** — decoded size is capped at 1MiB (hard validation error above), and above 512KiB the gateway logs a WARN at load time. See [Choosing a source form](#choosing-a-source-form). |
| `source.url` | String | — | An `http://` or `https://` URL the gateway fetches the bytecode from. **Requires `sha256`.** Connection behavior is tunable via `source.fetch`. |
| `source.fetch` | Object | — | Optional fetch hardening for `url` sources (invalid with `file`/`inline`/`oci`): TLS, auth, timeout, retry. See below. |
| `source.fetch.tls` | Object | — | The shared `TlsConfig` block: custom CA bundle (`validation.caCertificateRefs`) and/or client identity (`clientCertificateRef`) for mTLS to the artifact repository. Secret refs are resolved by the controller (cross-namespace refs need a `ReferenceGrant`). |
| `source.fetch.authHeaderSecretRef` | Object | — | `{name, namespace?, key}` of a Secret whose `key` (default `authorization`) holds the full `Authorization` header value sent with the fetch (e.g. `Bearer <token>`). If the ref is set but the Secret is not yet resolved, the load **fails** rather than silently fetching unauthenticated. |
| `source.fetch.timeoutMs` | Integer | `10000` | Per-attempt fetch timeout. Range `1..=60000`. |
| `source.fetch.retry` | Object | — | `{attempts, backoffMs}` — total attempts including the first (`1..=10`, default 3) and base backoff doubled per retry (default 500, max 30000). Fixed policy: transport errors and `5xx` retry; **`4xx` never retries**. |
| `source.oci` | Object | — | Pull the module from an OCI registry: `{image, pullPolicy?, pullSecretRef?}`. See [`source.oci` — pull from an OCI registry](#sourceoci--pull-from-an-oci-registry). |
| `sha256` | String (64 hex chars) | — | SHA-256 of the raw bytecode, verified before compilation. Optional for `file` / `inline` / `oci`; **required** for `url`. |
| `pluginConfig` | Raw bytes object | — | Opaque bytes passed to `proxy_on_configure`, readable via the `PluginConfiguration` buffer. See [Guest configuration bytes](#guest-configuration-bytes). |
| `vmConfig` | Raw bytes object | — | Opaque bytes passed to `proxy_on_vm_start`, readable via the `VmConfiguration` buffer. See [Guest configuration bytes](#guest-configuration-bytes). |
| `failOpen` | Boolean | `false` | Governs behavior on load failure / trap / timeout / Body access failure / callout no-resume — see [Failure semantics](#failure-semantics). |
| `timeoutMs` | Integer | `100` | Per-hook wall-clock budget in milliseconds (epoch interruption). Range `1..=10000`. Body-chunk hooks run at `timeoutMs / 10`. |
| `instancePoolSize` | Integer | `8` | Max pooled instances kept per compiled module, per gateway process. Range `1..=64`. |
| `calloutAllowlist` | String list | `[]` | Origins (`scheme://host[:port]`) the guest may target with `proxy_http_call`. **Empty = callouts disabled** (dispatch fails with a host error the guest can observe). Exact-origin match after normalization. |
| `calloutTimeoutMs` | Integer | `5000` | Upper bound on each callout's guest-supplied timeout (also the effective timeout when the guest passes 0). Range `1..=10000`. |

### Guest configuration bytes

`pluginConfig` and `vmConfig` each accept exactly one of these object shapes:

```yaml
pluginConfig:
  string: "plain UTF-8"
vmConfig:
  base64: "/wA=" # arbitrary bytes, including non-UTF-8
```

Use `{json: <value>}` for structured data. Any JSON value is valid, including an
object, array, string, number, boolean, or null. Edgion serializes the parsed
value as compact UTF-8 JSON with object keys sorted recursively. This canonical
form is what the guest receives and what the module-cache identity hashes;
original whitespace, key order, and number spelling are not preserved.

The three forms are strictly mutually exclusive. A missing selector, multiple
selectors, unknown fields, a bare legacy string, non-canonical Standard padded
base64, or a decoded value above 64KiB is rejected. Absence differs from an
explicit empty value. Different forms that produce the same bytes intentionally
share the same initialized module-cache entry.

## Choosing a source form

| Form | Position | Size bound | Distribution path |
|---|---|---|---|
| `inline` | **Dev/test convenience.** Not a production distribution mechanism. | 1MiB decoded (hard cap); K8s object + etcd limits apply on top | Rides the CR through etcd; every conf_sync carries it in full |
| `file` | Modules baked into the gateway image or mounted via initContainer | 64MiB hard cap | Image build / volume mount |
| `url` | **Recommended for production** — internal artifact repository (Nexus, Artifactory, S3 presigned) | 64MiB hard cap | Gateway fetches directly, with `source.fetch` TLS/auth/retry and mandatory `sha256` |
| `oci` | **Recommended for production** — Istio-style OCI image distribution | 64MiB blob and extracted-module hard caps | Gateway pulls directly from an OCI registry, with a content-addressed disk cache |

This mirrors Istio's posture: Istio's `WasmPlugin` has **no inline field** at all and treats `oci://` as the primary channel with `http(s)` as the second. Edgion keeps `inline` for fast iteration but caps and warns on it; a ConfigMap-based form was evaluated and rejected (same etcd object cap as inline, plus ref-tracking complexity, no added value).

### `source.oci` — pull from an OCI registry

```yaml
source:
  oci:
    image: registry.internal/wasm/header-demo:1.2.0   # or ...@sha256:<digest>
    pullPolicy: IfNotPresent                          # optional: IfNotPresent | Always
    pullSecretRef:                                    # optional, private registries
      name: registry-creds                            # kubernetes.io/dockerconfigjson Secret
```

| Field | Notes |
|---|---|
| `image` | `registry/repo:tag` or `registry/repo@sha256:<digest>`. The registry host is required. |
| `pullPolicy` | Unset follows the Kubernetes convention: `Always` for `latest` / missing tag, `IfNotPresent` otherwise. A digest reference is always pinned. `Always` re-resolves the tag's digest at most every 60s and reloads only when it changed. |
| `pullSecretRef` | A `kubernetes.io/dockerconfigjson` Secret. Cross-namespace references require a ReferenceGrant. If the Secret cannot be resolved, the plugin load fails — there is no silent anonymous fallback. |

Accepted image shapes (Istio parity): a layer with media type
`application/vnd.module.wasm.content.layer.v1+wasm` (pushed by `oras`), or a
single tar+gzip layer containing `plugin.wasm` (plain `docker build` from
scratch). `sha256` stays optional for OCI sources — the digest chain already
covers transport integrity — and, when set, is verified against the final
wasm bytes as for every other source.

Pulled manifests and blobs are cached on local disk (default
`wasm-oci-cache/` under the gateway work directory; recommend an emptyDir
mount in Kubernetes) so a gateway restart does not re-download pinned
content. The cache is bounded: entries unused for 24h are purged, and a
total-size budget (default 512MiB) is enforced on every write with LRU
eviction. Pull timeout and retry attempts are tunable too. See the gateway
config fields `wasm_oci_cache_dir`, `wasm_oci_cache_ttl_secs`,
`wasm_oci_cache_purge_interval_secs`, `wasm_oci_cache_max_bytes`,
`wasm_oci_pull_timeout_secs`, `wasm_oci_pull_attempts`.

Limitations (v1): registries must present a publicly-verifiable TLS
certificate (loopback registries use plain HTTP for local testing); no
registry mirrors. For static deployments, `source.file` with an
initContainer (or the Kubernetes `image` volume source) remains the
kubelet-native alternative — `source.oci` is for dynamically applied plugins
that must hot-load without a pod restart.

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

`proxy_http_call` (what the official proxy-wasm SDKs emit; the host also accepts the older `proxy_dispatch_http_call` spelling) lets the guest call an external HTTP service mid-request — the proxy-wasm ecosystem's most common plugin shape (external auth, risk scoring, audit). Flow:

1. In `on_http_request_headers`, the guest dispatches one or more callouts and returns `Action::Pause`.
2. The gateway executes the callouts (asynchronously, off the guest's CPU budget) while the request is suspended.
3. Each result is delivered via `proxy_on_http_call_response`; the guest reads `HttpCallResponseHeaders` / `HttpCallResponseBody` and either resumes the request (`resume_http_request` / `proxy_continue_request`) or answers directly (`send_http_response` / `proxy_send_local_response`).

Semantics and guardrails:

- **Addressing**: the `upstream` parameter is the target **origin URL** (`https://auth.internal:8443`), not an Envoy cluster name. The `:path`, `:method`, and `:authority` pseudo headers from the dispatched header map drive the outbound request (`:authority` is sent as `Host`).
- **Allowlist**: the origin must exactly match an entry in `calloutAllowlist` (after normalization). No allowlist = no callouts. A denied dispatch returns `INTERNAL_FAILURE` from `proxy_http_call` synchronously.
- **Request and tick phases**: the response-headers/body stages are synchronous and cannot await; dispatch there returns `UNIMPLEMENTED`. Callouts from `on_tick` are supported with root-context callbacks — see [Timers](#shared-data-and-timers) for the extra allowlist-agreement condition.
- **Pause required** (request phase): `dispatch` + `Action::Continue` is not supported — the queued callouts are dropped and logged (`callouts-dropped=N`). Dispatch must be followed by `Pause`. (`on_tick` has no Pause concept: callouts dispatched there are always executed after the hook returns.)
- **Budgets**: at most 8 dispatches per request (or per tick), 4 callback rounds (a callback may dispatch follow-ups), 64 concurrent callouts process-wide, 256KiB response-body cap per callout. Each callout's timeout is `min(guest_timeout, calloutTimeoutMs)`.
- **Callout failure** (connect error, timeout, oversized body): the callback fires with **0 headers** — the proxy-wasm convention — and the guest decides its own fail behavior. The host never traps the guest for a failed callout.
- **Guest never resumes**: if after all callouts the guest neither continued nor answered, the host applies `failOpen` (reason `no-resume`).
- **Epoch budget**: `timeoutMs` bounds each *guest* hook invocation; the callout's network wait does not consume guest CPU budget.

### Callout vs ExtProc

Both integrate external decision services; they differ in direction:

- **ExtProc** (push): the gateway pushes every request through a resident external processor over gRPC. Choose it when the external service owns the logic and sees all traffic.
- **Wasm callout** (pull): guest logic lives in the gateway and calls out on demand. Choose it when the plugin decides *if/when* to consult an external service (e.g. cache-then-verify token checks).

## Two body modes, chosen statically

Edgion supports two, deliberately different, wasm body behaviors:

| | Request-body buffer mode | Response-body observation |
|---|---|---|
| Hook | `proxy_on_request_body` | `proxy_on_response_body` |
| Entry stage | `requestPlugins` (same entry as the headers hook) | `upstreamResponseBodyFilterPlugins` |
| `body_size` | **Whole body** length | **Current chunk's** length |
| `proxy_get_buffer_bytes` returns | The **whole buffered body** | Only the **current chunk** |
| Can deny the request? | ✅ (`proxy_send_local_response`) | ❌ (headers already sent) |
| Can modify the body? | ✅ (`proxy_set_buffer_bytes`) | ❌ |
| How selected | Static: the compiled guest module exports `proxy_on_request_body`; an optional `body:` block adds pre-execution body policy | Static: whether the guest module exports `proxy_on_response_body` |

**This is a deliberate deviation from the proxy-wasm spec.** The spec models buffering as a *dynamic*, per-chunk decision the guest makes at runtime (`if !end_of_stream { Action::Pause }` tells the host to keep buffering; `Action::Continue` tells it to stream). Envoy implements this by mapping `Pause` onto `StopIterationAndBuffer` and letting its filter manager own the buffer. Edgion cannot do that on either side — the response-body filter is a **synchronous**, immutable-body hand-off (`&Option<Bytes>` in, `Option<Duration>` out; no way to withhold or accumulate a chunk), and the decision of "does this plugin get the whole body" has to be made before any hook runs, once per module load. So Edgion selects the mode **once, statically**, from which hook the guest module exports and which stage the operator wired it at — not from the guest's per-request `Action` return value.

### Request-body buffer mode

A Wasm entry gets whole-request-body mode when it is listed under `requestPlugins` and its guest exports `proxy_on_request_body`. Without a `body:` block, capture happens lazily after `proxy_on_request_headers` returns `Continue` (that is, it did not already deny or trap). The gateway then invokes `proxy_on_request_body` exactly **once** with `body_size` = the full body length, `end_of_stream = true`, and `proxy_get_buffer_bytes(HttpRequestBody, 0, body_size)` returning the whole thing. The guest can inspect the full body — for a request-body WAF, secret scanner, or signature check — deny the request via `proxy_send_local_response`, or rewrite it with `proxy_set_buffer_bytes(HttpRequestBody, start, size, bytes)`.

Notes and current limits:

- **Optional policy gate via `body:`.** The exported hook enables request capture; `body:` adds a generic gate before the Wasm request-header hook. Its `maxBodySize` is a **per-plugin policy limit** on top of the gateway physical cap, not a buffer allocation size: an over-limit request returns `413` with the default `onReadFailure: failClose`, while `failOpen` skips the whole Wasm entry. Its `conditions` decide whether that pre-gate policy runs; when they skip, the inner Wasm plugin still captures later because the exported hook remains active. A `body:` block on a guest without `proxy_on_request_body` captures before the request-header hook even though no body hook will run.
- **Cap**: the buffer is capped by the resolved `EdgionGatewayConfig.spec.requestBody` policy, not a per-plugin allocation value.
- **Shared, bounded capture.** The mode uses the same request-scoped body store as other whole-body request plugins. Capture is lazy, bounded by the gateway physical cap, spills from memory to disk when needed, and exposes the current immutable body version produced by earlier plugins. The gateway physical cap and refusal to buffer an ineligible request are always fail-close.
- **Strict range semantics.** The range `[start, start + size)` must be fully contained in the current staged body. A zero-length range inserts; `start == body length` with `size == 0` appends; empty replacement bytes delete the selected range. An invalid range returns `BAD_ARGUMENT`, and exceeding the physical body cap returns `INTERNAL_FAILURE`.
- **Bounded mutation work.** A request-body hook may perform cumulative mutation work equal to two passes over the resolved gateway physical body cap. Each operation charges its replacement bytes and, when the edit changes length, the tail bytes that must be moved. This allows normal range edits and up to two complete equal-size rewrites at any configured body cap, while preventing repeated rewrites or front insert/delete loops from amplifying CPU and memory-bandwidth use. A smaller physical cap proportionally reduces this allowance. An operation that would exceed the budget returns `INTERNAL_FAILURE` without changing the staged body.
- **SDK error behavior.** The host returns those ABI statuses without deliberately trapping the guest. Some high-level proxy-wasm SDK helpers, including Rust's `set_http_request_body`, panic when a host call returns a non-OK status; validate ranges and output size in the guest when using such helpers.
- **Atomic publication.** All successful writes made during one hook update a private staging buffer. A rejected host call leaves that staging buffer unchanged by that call. After a continuing hook returns, the gateway publishes the final bytes to the shared request-body store in one transaction; a trap or publication failure cannot expose a partially published body. A local response skips publication because the proxied request has ended. Later plugins, request mirroring, retries, and upstream replay all consume the committed generation; upstream framing is recalculated from its final length.
- **Capture timing depends on `body:`.** With `body:`, the generic gate captures before any Wasm hook and applies its policy. Without `body:`, the request-header hook runs first; only a clean `Continue` proceeds to lazy capture and the body hook. In both cases the body hook sees the same complete request version. A guest that does not export `proxy_on_request_body` skips the body phase entirely.

Minimal shape of a request-body guest:

```rust
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::Action;

impl HttpContext for BodyScanner {
    fn on_http_request_headers(&mut self, _n: usize, _eos: bool) -> Action {
        Action::Continue // let the request through to the body hook
    }

    // Invoked once, with the WHOLE request body (Edgion buffers it for you —
    // no need for the classic `if !end_of_stream { Pause }` dance).
    fn on_http_request_body(&mut self, body_size: usize, _end_of_stream: bool) -> Action {
        if let Some(body) = self.get_http_request_body(0, body_size) {
            if body.windows(6).any(|w| w == b"secret") {
                self.send_http_response(403, vec![], Some(b"request body denied"));
            }
        }
        Action::Continue
    }
}
```

### Response-body observation (read-only)

Listing a Wasm entry under `upstreamResponseBodyFilterPlugins` invokes `proxy_on_response_body` once per upstream chunk. This is the **only** wasm body mode on the response side: the main upstream response remains streaming and has no whole-response buffer mode (see [Remaining limitations](#remaining-limitations)).

> ### ⚠️ Per-chunk, not whole-body — read before porting a guest
>
> `body_size` is the **current chunk's** length, `proxy_get_buffer_bytes(HttpResponseBody, …)` returns **only that chunk**, and the body is never accumulated across chunks.
>
> **Consequence:** a guest written to the dynamic buffering idiom —
> ```rust
> if !end_of_stream { return Action::Pause; }       // expects the host to buffer
> let body = self.get_http_response_body(0, body_size)?;  // expects the WHOLE body
> ```
> — would observe **only the last chunk**. For a WAF or secret scanner that is a silent false negative. The host therefore increments `edgion_wasm_body_pause_unsupported_total{module}` whenever `Pause` cannot be honored. This bounded metric is the only signal on the per-chunk data-plane path; no per-module WARN is emitted. **Alert on that counter** — it reliably indicates that a deployed guest expects whole-body buffering. Whole-body mode is available only on the *request* side and is selected by an exported `proxy_on_request_body` hook; an optional `body:` block adds policy. The main upstream response remains streaming-only.

Working within the per-chunk model:

- Cross-chunk aggregation is the **guest's** job (its own memory, or [shared data](#shared-data-and-timers) — see the reference guest, which sums each chunk's byte count into a shared counter).
- **Strictly read-only**: the body cannot be modified, and the response cannot be short-circuited from the body phase (headers are already sent). A detection verdict can only be logged (`proxy_log`) or aggregated via shared data.
- Each chunk runs under a reduced CPU budget (`timeoutMs / 10`, floor 10ms) so a slow guest cannot stall a long stream; a trap in the body hook **never interrupts the stream** (the instance is discarded and streaming continues, regardless of `failOpen`).
- A guest that does not export `proxy_on_response_body` is a free no-op at this stage.

## Shared data and timers

### Shared data

`proxy_get_shared_data` / `proxy_set_shared_data` back a per-module KV store with proxy-wasm CAS semantics (per-key monotonically increasing CAS token; `set` with a nonzero expected CAS fails with `CasMismatch` on mismatch; CAS 0 writes unconditionally; a null value deletes the key).

- **Scope: one module (cache entry) within one gateway process.** All pooled instances of the module share it — across requests, phases, and the tick task. It is **not** cluster-level state: for cluster-wide coordination (e.g. global rate limiting) use `rate_limit_redis`, not shared data.
- **Trust implication:** every `EdgionPlugins` entry whose source resolves to the same bytecode shares this store — including data a tick-phase callout fetched under one plugin's allowlist, which requests flowing through the *other* plugins can then read. Don't share one module across trust domains whose data must stay separated; ship distinct bytecode (or distinct sources) per tenant instead.
- Limits: 64KiB per value, 4MiB total per module. An oversized write returns `BAD_ARGUMENT`. The total is accounted as key + value bytes **plus a fixed ~128B per-entry overhead** (approximating real memory cost), so workloads with many tiny keys get proportionally less logical space — the cap bounds actual memory, not just payload bytes.
- Lifecycle: freed when the module's cache entry is evicted (see [Module cache lifecycle](#module-cache-lifecycle)); a reload that changes the module content starts with an empty store.

### Timers (`on_tick`)

A guest that calls `proxy_set_tick_period_milliseconds(N)` (typically from `on_configure`) gets `proxy_on_tick` invoked every `N` ms (floor 100ms) on a background task, one per module. The tick instance is leased from the same pool as request instances and runs under the normal `timeoutMs` epoch budget, and **consecutive ticks may run on different pooled instances** — root-context struct fields are per-instance, so state that must survive across ticks belongs in shared data (state *initialized* in `on_configure`, like a parsed config, is fine: every pooled instance runs `on_configure` at init). The guest can change the period or stop ticking (`0`) from inside `on_tick`. The tick task stops automatically when the module's cache entry is evicted.

Typical pattern: `on_tick` dispatches a callout to fetch the latest ruleset and writes it into shared data; request hooks read it (rate-limit rule refresh, denylist sync, config polling).

#### Callouts from `on_tick`

`on_tick` may dispatch HTTP callouts like the request phase does. Differences:

- **No Pause/resume**: `proxy_on_tick` has no Action return; callouts queued during the hook are executed right after it returns, and each result is delivered via `proxy_on_http_call_response` on the **root context** (which lives as long as the instance — no per-request context is involved).
- **Allowlist agreement required.** The tick task is *module-level*, but `calloutAllowlist` is *per-plugin* — and several `EdgionPlugins` entries pointing at the same bytecode share one module (and one tick task). The tick phase therefore arms dispatch only when the plugins referencing the module agree on **one** non-empty allowlist:
  - Entries with an **empty** `calloutAllowlist` (e.g. the same module also wired at a response/body stage without callout config) carry no opinion and never block tick callouts.
  - Two or more **different** non-empty allowlists on one module are a **conflict**: tick dispatch stays disabled (`UNIMPLEMENTED`, same as before the feature), a WARN is logged, and `edgion_wasm_callout_total{result="tick_config_conflict"}` is incremented each tick. Fail-safe by design — the alternative would let one plugin's allowlist leak into another's tick execution.
  - The conflict heals automatically once the disagreeing plugin is removed or its allowlist aligned.
- The effective timeout cap is the **minimum** `calloutTimeoutMs` across the plugins granting the allowlist; all other callout guardrails (8 dispatches per tick, 4 rounds, concurrency, body cap) apply unchanged.

## Failure semantics

All failure modes route through the same `failOpen` decision:

| Reason | Trigger | PluginLog |
|---|---|---|
| `load` | The module fails to download, decode, compatibility-check, or compile; its `sha256` does not match; or instance init fails (`on_vm_start`/`on_configure` returned false). Load errors are cached for 30s. | `wasm load:fail-open` / `fail-close` |
| `acquire` | The instance pool cannot produce a usable VM at request time (e.g. pooling-allocator slots exhausted). | `wasm acquire:…` |
| `trap` | The guest panics, hits `unreachable`, blows its CPU budget, or otherwise faults while running a hook (including the callout callback and context close). | `wasm trap:…` |
| `callout` | Callout resolution failed structurally (e.g. the guest's callback trapped mid-delivery). | `wasm callout:…` |
| `no-resume` | The guest paused on callouts but neither resumed nor answered after all of them completed. | `wasm no-resume:…` |
| `body read` | The complete request Body could not be read after capture was admitted. A stage refusal and a capture storage failure remain unconditionally fail-close. | `wasm body read:…` |
| `body mutation` | Publishing the guest's final staged request Body failed. Neither its Body nor its request-header operations are partially committed. | `wasm body mutation:…` |
| `not loaded` | Response-phase only: the sync hook uses a non-blocking cache lookup; if the module hasn't finished its (async) first load, this fires and a background load is kicked off. | `wasm not loaded:…` |

- **`failOpen: false`** (default) — the request is short-circuited with `500` and body `wasm plugin error`.
- **`failOpen: true`** — the failure is logged and the plugin chain continues.

### Trap poisoning and recovery

A trapped (or otherwise faulted) VM instance is **poisoned**: it is discarded rather than returned to the instance pool, freeing its slot. The next request gets a fresh instance re-instantiated from the cached compiled module (cheap — only `Module::new` is expensive). A guest panic never crashes, hangs, or otherwise impairs the gateway process.

### Timeout

Each hook call is bounded by `timeoutMs` via wasmtime **epoch interruption**: a background ticker increments the engine epoch every 10ms, and the deadline is re-armed at the start of *every* guest call (headers hooks, callout callbacks, body chunks, ticks). Effective granularity is ~10-20ms. Callout network waits happen *between* guest calls and never consume guest budget.

### Host-side resource limits

`timeoutMs` bounds execution *time*, not host *memory*. Two additional limits bound what a single hook can allocate on the host so a buggy or hostile guest cannot drive the process toward OOM (which would affect unrelated routes and tenants — `failOpen` cannot recover from process-wide memory exhaustion):

- **Per-phase ABI retention budget** — the host-retained bytes a guest accumulates through repeated ABI calls in one hook (queued header ops, `proxy_log` lines, `proxy_set_property` entries) are capped at **32 MiB / 200,000 operations**. Past the budget, further ABI calls return an error status and make **no** partial change; the accumulators are cleared when the hook returns. Legitimate header/log/property use is orders of magnitude below this.
- **Guest linear-memory cap** — guest memory growth is capped at **128 MiB** (and tables at 100,000 entries), well above any real proxy-wasm working set. This also bounds the size of any single value the guest hands to a host call.

## Module cache lifecycle

The compiled-module cache is keyed by the source **plus, for `file` sources, the file's size+mtime**:

- **Refcount eviction.** Every live Wasm plugin instance pins its cache entry. When a reload/config change drops the last reference to an entry (e.g. inline content changed → new key), the orphaned entry is evicted after a **60s grace period** (which absorbs reload thrash). In-flight requests keep the entry alive via their own strong references; memory is freed when the last one finishes. Evictions show up in the `edgion_wasm_module_evicted_total` metric.
- **`file` sources are content-aware.** The runtime cache key includes the file's size and mtime, captured when the plugin is (re)built. Overwriting the `.wasm` at the same path **and triggering a plugin reload** (any change to the owning `EdgionPlugins` resource, or a full config resync) picks up the new content; the old entry is refcount-evicted. Without a reload the running plugin keeps its original key — a bare file overwrite alone still doesn't hot-swap.
- **Eager instance warm-up.** One instance is instantiated at load time, so guest init failures surface as clear load errors (negative-cached) instead of per-request acquire failures, and a tick period requested during `on_configure` starts ticking before the first request.
- **Multi-tenant sizing.** All Wasm modules share one engine-wide pooling allocator. Size `instancePoolSize` with the total across all configured Wasm plugins in mind — estimate the peak instance count as roughly `edgion_wasm_modules_loaded × instancePoolSize`.

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
| `edgion_wasm_hook_duration_ms` | histogram | `phase` | Guest hook wall time (`request_headers`, `request_body`, `response_headers`, `response_body`, `http_call_response`, `tick`) |
| `edgion_wasm_trap_total` | counter | `phase` | Traps / protocol violations |
| `edgion_wasm_fail_mode_total` | counter | `reason`, `mode=open\|close` | Fail-mode decisions |
| `edgion_wasm_callout_total` | counter | `result=ok\|error\|denied\|over_limit\|tick_config_conflict` | Callout outcomes; `tick_config_conflict` counts ticks where dispatch stayed disabled because plugins sharing the module configure different `calloutAllowlist`s |
| `edgion_wasm_modules_loaded` | gauge | | Compiled modules currently in the global cache |
| `edgion_wasm_shared_data_bytes` | gauge | `module` | Current shared-data total per module, keys + values + ~128B/entry overhead (cap 4MiB) |
| `edgion_wasm_body_pause_unsupported_total` | counter | `module` | **Alert on this.** A guest returned `Action::Pause` from the **response-body observation** hook — it wants whole-body buffering, which is not available on the response side (only on the request side, via [Request-body buffer mode](#request-body-buffer-mode)) — and is silently observing only the current chunk instead |

A trap storm shows up as `edgion_wasm_trap_total` and `edgion_wasm_instance_discard_total{reason="poisoned"}` rising together.

## `PluginLog` format

- `wasm ok ops=N logs=M` — success; `N` header operations applied, `M` guest log lines (content goes to the debug log).
- `wasm ok ops=N pause-ignored` — the guest returned `Pause` without dispatching a callout or sending a local response; the request continues.
- `wasm callouts=N leftover=M` — request phase resolved `N` callouts (`M` dropped by the round cap).
- `wasm callouts-dropped=N` — the guest dispatched callouts but returned `Continue`; they were dropped (unsupported pattern).
- `wasm local <status>` — the guest sent a local response.
- `wasm <reason>:fail-open` / `wasm <reason>:fail-close` — one of the failure reasons above.

## Remaining limitations

- **Request-body buffer mode is selected by the exported hook**: the shared runtime lazily captures the complete request behind the gateway physical cap. An optional `body:` block adds pre-execution conditions, a per-plugin `maxBodySize`, and `onReadFailure` handling; it is not the capture switch. See [Request-body buffer mode](#request-body-buffer-mode).
- **No response-body buffer mode**: the main upstream response remains streaming. Wasm response-body observation is per-chunk and read-only — no body rewrite, no short-circuit from the body phase, and `Action::Pause` cannot be honored — so a guest written to the standard whole-body buffering idiom sees only the current chunk. See the warning in [Response-body observation](#response-body-observation-read-only).
- **Request-body rewrite only**: `proxy_set_buffer_bytes(HttpRequestBody, ...)` is valid only during the whole request-body hook. Response-body writes remain `STATUS_UNIMPLEMENTED`; other phases return a defined ABI error.
- **Static mode selection, not dynamic Pause/Continue**: unlike Envoy, the body mode a plugin gets is fixed at module-load time by which hooks the compiled guest exports, not chosen per-request by the guest's `Action` return value. See [Two body modes, chosen statically](#two-body-modes-chosen-statically).
- **No gRPC callouts** (`proxy_dispatch_grpc_call`): calling it traps.
- **No callouts from the response/body phases** (sync stages), and request-phase dispatch without `Pause` is dropped. Tick-phase callouts additionally require the module's plugins to agree on one `calloutAllowlist` (see [Timers](#shared-data-and-timers)).
- **No shared queues, foreign functions, or guest-defined metrics**: calling them traps.
- **Per-request contexts are per-hook-call**: the request-headers and response-headers hooks of the same request may run on different pooled instances with fresh context ids. Correlate the phases through shared data or a stashed request header, not guest struct fields.
- **`source.oci` v1 constraints**: registries must present a publicly-verifiable TLS certificate (loopback registries may use plain HTTP for local testing); no registry mirrors. See [`source.oci` — pull from an OCI registry](#sourceoci--pull-from-an-oci-registry).

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

See `edgion-tests/wasm/README.md` for the full contract the reference guest implements and for how to rebuild the committed artifact after editing the guest source. The reference guest exercises both whole-request-body mode (`on_http_request_body`) and per-chunk response-body observation (`on_http_response_body`).

## FAQ

### Q: Which proxy-wasm ABI versions are supported?

`0.2.0` and `0.2.1` (the versions the Rust/C++ SDKs export as `proxy_abi_version_0_2_x`). An unsupported or mixed marker set fails the compatibility check; a module exporting none loads with a WARN when all imports and hooks are otherwise compatible.

### Q: My callout is never executed?

Check, in order: (1) the target origin is listed in `calloutAllowlist` — empty list disables callouts entirely; (2) for request-headers-phase dispatch, the guest returns `Action::Pause` — dispatch followed by `Continue` is dropped (`callouts-dropped=N` in the access log); (3) the dispatch happens in the request-headers or tick phase — request-body, response-headers, and response-body phases return `UNIMPLEMENTED`; (4) for tick-phase dispatch, no allowlist conflict — check `edgion_wasm_callout_total{result="tick_config_conflict"}` and the WARN log when several plugins share the module with different `calloutAllowlist`s.

### Q: My request is failing with an unexpected `500` instead of running `proxy_on_request_body`?

Confirm that the `Wasm` entry is under `requestPlugins` and the compiled guest actually exports `proxy_on_request_body`. Then check the gateway physical cap and request eligibility (gRPC and body-carrying upgrades are refused). If the entry has `body:`, also check its `conditions`, `maxBodySize`, and `onReadFailure`: fail-close limit/read failures reject, while fail-open policy failures skip the entire entry. Admission refusal and the gateway physical cap never fail open.

### Q: My guest module isn't taking effect on the response phase?

Confirm you listed the entry under `upstreamResponseFilterPlugins` (headers) or `upstreamResponseBodyFilterPlugins` (read-only body), not `upstreamResponsePlugins`. ExtProc is configured once in `requestPlugins`; its response work is an internal companion owned by that request entry, not a separately configurable async-stage entry.

### Q: The controller rejected my EdgionPlugins resource?

Common causes: `source` has zero or more than one of `file`/`inline`/`url`/`oci` set; `source.url` without `sha256`; `sha256` isn't 64 hex chars; `source.inline` isn't valid base64 or decodes to more than 1MiB; `source.fetch` without `source.url`; `source.oci.image` isn't a valid `registry/repo:tag` or `registry/repo@sha256:<digest>` reference; `calloutAllowlist` entries that aren't clean `scheme://host[:port]` origins; `timeoutMs`/`calloutTimeoutMs` out of `1..=10000`; `instancePoolSize` out of `1..=64`.

### Q: I replaced the `.wasm` file on disk but the gateway still runs the old build?

The content-aware cache key is captured when the plugin is built. Touch the owning `EdgionPlugins` resource (any update) after overwriting the file — the reload re-stats the file, lands on a new cache entry, and the old one is evicted after the grace period.

### Q: Can two requests share state?

Yes — via `proxy_{get,set}_shared_data`, which is shared across all instances of the same module **within one gateway process**. It is not replicated across gateway replicas; cluster-level coordination still belongs to `rate_limit_redis`.

## Related links

- [ExtProc Plugin](./ext-proc.md) — the gRPC push-model alternative (see [Callout vs ExtProc](#callout-vs-extproc))
- [Filters overview](../README.md)
