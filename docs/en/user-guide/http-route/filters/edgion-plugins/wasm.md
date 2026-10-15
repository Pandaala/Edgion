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
> Wasm executes a [proxy-wasm](https://github.com/proxy-wasm/spec) ABI guest module — a core WebAssembly binary satisfying the Edgion HTTP profile below — directly inside the gateway process via [wasmtime](https://wasmtime.dev/). It is provided by the `EdgionPlugins` CRD and is not part of standard Gateway API.

## What does Wasm solve?

Proxy-Wasm defines a proxy extension ABI. Binary imports, SDK initialization and host behavior must all match; sharing an ABI marker with Envoy or Higress does not establish compatibility. Instead of writing a native Rust plugin against Edgion's internal APIs, you compile a small WebAssembly module against the [proxy-wasm SDK](https://crates.io/crates/proxy-wasm) for your language of choice and hand the compiled `.wasm` bytecode to this plugin. Each plugin runtime compiles its loaded generation once and pools initialized instances for reuse across requests. Rebuilding the plugin may repeat compilation.

The implemented ABI is a deliberate subset (see below): header read/write, local responses, **HTTP callouts** (request phase with Pause/resume, and from `on_tick`), **request-body buffer mode** (whole-body, guest-declared, can deny or rewrite), **read-only response-body observation** (per-chunk), **shared data**, and **timers** (`on_tick`). Whole-response buffering and response-body modification are not supported: the main upstream response always remains streaming.

## Request-owned execution and response hooks

```yaml
requestPlugins:
  - type: Wasm
    config:
      source:
        file: plugins/example.wasm
      maxActiveRequests: 32
      response:
        headers: true
        body: true
        trailers: true
```

All three response flags default to false and are valid only on `requestPlugins`.
One request retains the exact loaded generation, exclusive VM, and HTTP context
from request headers through final cleanup. Request body and opted-in response
hooks see the same guest object; overlapping requests have different instances.
A request admits at most 64 bindings and one binding per constructed entry;
a repeated installation uses the configured failure mode.
The final async upstream-header companion can mutate headers or reply locally.
The streaming body and trailer companions are observational: mutation, local
responses, logging and Pause are unsupported. There is no response accumulation.
Before-trailers is not EOS; real trailers close the stream, and a headers-only
response does not receive a duplicate synthetic body EOS after a header EOS.

Host callback snapshots are released after each callback, even while the guest
context stays alive. Normal request termination invokes done/delete once in
the universal logging hook. Cancellation of an execution future or dropping the request owner discards
the execution without guest code from Drop. A healthy transport-error terminal
that reaches logging still runs normal cleanup. A skipped request entry never runs its response companion.
This bounded exclusive model intentionally trades instance occupancy for simple
ownership; it is not a high-density multi-context runtime.

## Named compatibility profile

`edgion-proxy-wasm-http-v1` is the bounded HTTP subset described on this page.
`edgion-cli wasm inspect` report version 2 identifies this profile and labels its
assessment `static_load_compatibility`. A successful result proves only that the
binary passes static analysis: it does not execute the guest, certify its SDK,
or guarantee instantiation under the configured runtime limits.

| Layer | Contract |
|---|---|
| Binary | Core Wasm, exact `env` function signatures; ABI markers 0.2.0/0.2.1. Missing marker warns; mixed/unknown markers reject. No WASI or components. |
| SDK | The reference Rust guests pin `proxy-wasm` 0.2.5 and `wasm32-unknown-unknown`. Other SDK/compiler/target combinations need their own build and execution evidence. |
| Host calls | `implemented` is subject to phase/argument restrictions below; `accepted_noop` returns OK without semantics; `known_unsupported_trap` traps if called. |
| Hooks | `known_not_invoked` exports load but are never called. Response trailers require an attached request entry with `response.trailers: true`. |
| Behavioral evidence | Fixture compilation, static acceptance and runtime assertions are separate results. See the [fixture inventory](../../../../../../edgion-tests/wasm/compatibility-profile.md). |

TinyGo and upstream Go are separate toolchains. The upstream Go SDK requires
Go 1.24+ WASI reactors; its WASI imports are outside this profile. C++ is not
certified merely because its ABI marker matches. Envoy cluster names, arbitrary
Envoy properties and Higress extensions are not emulated. A future narrowly
scoped WASI adapter is a separate decision from WASI HTTP components and must
specify initialization, clocks/randomness, environment, filesystem and scheduling
capabilities explicitly.

## Supported ABI subset

### Hooks

| proxy-wasm hook | EdgionPlugins stage | When it runs |
|---|---|---|
| `proxy_on_context_create` / `proxy_on_vm_start` / `proxy_on_configure` | — (instance init) | Once per pooled instance, before any request is served |
| `proxy_on_request_headers` | `requestPlugins` (`RequestFilter`) | Request headers received, before the backend is contacted |
| `proxy_on_request_body` | `requestPlugins` (`RequestFilter`), same entry as headers | Once per request, **after** `proxy_on_request_headers`, only if the guest module exports this hook — see [Request-body buffer mode](#request-body-buffer-mode) |
| `proxy_on_http_call_response` | — (callout callback) | Once per completed callout: on the paused request context for callouts dispatched from `proxy_on_request_headers`, or on the root context for callouts dispatched from `proxy_on_tick` |
| `proxy_on_response_headers` | Explicit request `response.headers`, or standalone `upstreamResponseFilterPlugins` | Final upstream response headers |
| `proxy_on_response_body` | Explicit request `response.body`, or standalone `upstreamResponseBodyFilterPlugins` | Once per upstream body chunk, **read-only** (see [Response-body observation](#response-body-observation-read-only)) |
| `proxy_on_response_trailers` | Explicit request `response.trailers` | Real upstream trailers, read-only; bounded to 64KiB and 1024 pairs |
| `proxy_on_tick` | — (background) | Periodically, once the guest set a tick period (see [Timers](#shared-data-and-timers)) |
| `proxy_on_done` / `proxy_on_delete` | — (per-context cleanup) | After each per-request context finishes |

Wasm is not configured directly at `upstreamResponsePlugins`. A request entry
can explicitly attach response companions using `response` below; they share
its request execution and run at the final upstream-response stage.
If a module needs the complete request body, place it in `requestPlugins`.
`RequestBodyBuffer` can preserve a request body for supported later consumers
such as DSL, but response-stage Wasm host phases do not expose that cache.

### Host calls

| Host function | Supported | Notes |
|---|:---:|---|
| `proxy_get_header_map_pairs` / `proxy_get_header_map_value` | ✅ | Request headers, response headers, attached response trailers, and (inside the callout callback) `HttpCallResponseHeaders` — the callout's response headers plus a synthetic `:status` pair |
| `proxy_replace_header_map_value` / `proxy_add_header_map_value` / `proxy_remove_header_map_value` / `proxy_set_header_map_pairs` | ✅ | Set / add / remove / bulk-replace; applied to the real request/response after the hook returns. Header-map writes to the callout response map are rejected |
| `proxy_send_local_response` | ✅ | Short-circuits the request with a final HTTP status (`200`–`599`), body, and headers. An invalid status is replaced with `500`. The body is forwarded byte-for-byte, so a binary payload (an image, protobuf, ...) is delivered unchanged; an empty body produces a bodyless response. Also valid from inside `proxy_on_http_call_response` (deny after an auth callout) |
| `proxy_http_call` | ✅ (request-headers + tick phases) | See [HTTP callouts](#http-callouts-pauseresume). In the request-body, response-headers, and response-body phases it returns `UNIMPLEMENTED` (12); tick calls require the owning plugin's allowlist (see [Timers](#shared-data-and-timers)) |
| `proxy_continue_request` / `proxy_continue_stream(Request)` | ✅ | Resumes a request paused on a callout. `proxy_continue_response` remains an accepted no-op |
| `proxy_get_shared_data` / `proxy_set_shared_data` | ✅ | Per-module KV with CAS — see [Shared data](#shared-data-and-timers) |
| `proxy_set_tick_period_milliseconds` | ✅ | Arms this plugin instance's tick task; `0` stops it. Period floor 100ms |
| `proxy_get_property` / `proxy_set_property` | ✅ (basic subset) | Readable keys: `request.path`, `request.method`, `request.host`, `source.address`, `plugin_name` (fixed to `Wasm`), `response.code` (response phase only). `proxy_set_property` writes into the current phase's in-memory property map only |
| `proxy_get_buffer_bytes` | ✅ | `VmConfiguration`, `PluginConfiguration`, `HttpCallResponseBody` (inside the callout callback), `HttpRequestBody` (the **whole buffered request body** in the request-body hook — see [Request-body buffer mode](#request-body-buffer-mode)), and `HttpResponseBody` (the **current chunk window** in the response-body hook) |
| `proxy_set_buffer_bytes` | ✅ (request body only) | Replaces, inserts, appends, or deletes bytes in `HttpRequestBody` during the whole request-body hook. Response-body writes return `STATUS_UNIMPLEMENTED` (12) |
| `proxy_log` | ✅ | Guest text is retained only within the current hook's bounded in-memory diagnostics and then discarded; the access log records only the bounded line count, never the guest-controlled text |
| `proxy_get_current_time_nanoseconds` | ✅ | Wall-clock time |
| `proxy_get_configuration` (legacy 0.1) | ✅ | Returns the same bytes as the `PluginConfiguration` buffer |
| `proxy_set_effective_context` / `proxy_done` / `proxy_continue_response` | Accepted no-ops | Return `STATUS_OK` without doing anything |
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

In compatibility reports, `compatible: true` means **accepted by static analysis of this host profile**, not that every declared feature is implemented or every exported hook is invoked. Check the `support` field on imports and hooks, plus the warnings list. `known_unsupported_trap` imports are safe to leave unused but trap if called. Standard SDK hooks marked `known_not_invoked`—including request trailers, queue-ready, gRPC callbacks, foreign-function callbacks, and stream callbacks—are accepted because SDKs commonly export them by default, but Edgion never calls them. Response trailers are implemented only for explicitly selected attached callbacks and produce a conditional-hook warning. `accepted_noop` imports also warn; accepting their signatures does not implement context switching, deferred completion or response continuation.

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
| `source.fetch.tls` | Object | — | The shared `TlsConfig` block: custom CA bundle (`validation.caCertificateRefs`) and/or client identity (`clientCertificateRef`) for mTLS to the artifact repository. Secret and ConfigMap CA refs are resolved by the controller (cross-namespace refs need a kind-matching `ReferenceGrant`); a missing ConfigMap reports `ResolvedRefs=False/ConfigMapNotFound`. |
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
| `maxActiveRequests` | Integer | `32` | Live request leases per configured runtime, across OCI generations; range `1..=64`. |
| `response` | Object | All false | Request-only `{headers, body, trailers}` companion selection, sharing one HTTP context. |
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
form is what the guest receives;
original whitespace, key order, and number spelling are not preserved.

The three forms are strictly mutually exclusive. A missing selector, multiple
selectors, unknown fields, a bare legacy string, non-canonical Standard padded
base64, or a decoded value above 64KiB is rejected. Absence differs from an
explicit empty value. Different forms that produce the same bytes give the
guest equivalent configuration, but separate plugin entries retain independent runtimes.

## Choosing a source form

| Form | Position | Size bound | Distribution path |
|---|---|---|---|
| `inline` | **Dev/test convenience.** Not a production distribution mechanism. | 1MiB decoded (hard cap); K8s object + etcd limits apply on top | Rides the CR through etcd; every conf_sync carries it in full |
| `file` | Modules baked into the gateway image or mounted via initContainer | 64MiB hard cap | Image build / volume mount |
| `url` | **Recommended for production** — internal artifact repository (Nexus, Artifactory, S3 presigned) | 64MiB hard cap | Gateway fetches directly, with `source.fetch` TLS/auth/retry and mandatory `sha256` |
| `oci` | **Recommended for production** — Istio-style OCI image distribution | 64MiB blob and extracted-module hard caps | Gateway pulls directly from an OCI registry, with a bounded content-addressed process-memory cache |

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

Verified manifests and blobs are cached in process memory, keyed by content
digest. Defaults are 256 MiB of retained payload allocations and 256 entries.
LRU eviction makes room for new entries; entries idle for 24 hours expire,
with a background sweep every 60 seconds. Set `wasm_oci_cache_max_bytes` or
`wasm_oci_cache_max_entries` to `0` to disable retention. Capacity pressure
may bypass caching without failing an otherwise valid module load.

There is no OCI disk cache or cache-directory setting. A Gateway restart
requires downloading the artifacts again, including digest-pinned images.
Existing cache files from older builds are not read or automatically deleted.
Removing the last Wasm plugin clears the artifact cache. This does not evict
live compiled modules or interrupt requests holding a module reference.

The byte budget counts payload allocation capacity, including evicted bytes
still borrowed by loaders. It is not a limit on total Wasm or process memory:
up to four OCI acquisition pipelines can additionally hold bounded download
and extraction buffers, while compilation, VM instances, and guest memory
have separate lifetimes. Shared cache hits do not clone the full payload.
The manifest, blob, extracted-module, and token-response limits remain
4 MiB, 64 MiB, 64 MiB, and 64 KiB respectively.

Gateway configuration fields: `wasm_oci_cache_max_bytes`,
`wasm_oci_cache_max_entries`, `wasm_oci_cache_ttl_secs`,
`wasm_oci_cache_purge_interval_secs`, `wasm_oci_pull_timeout_secs`, and
`wasm_oci_pull_attempts`. Entry capacity above 65,536 is rejected; zero TTL
or sweep intervals are clamped to one second. Pull timeout and retries are
independent of the cache limits.

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
- **Request and tick phases**: the response-headers/body stages are synchronous and cannot await; dispatch there returns `UNIMPLEMENTED`. Callouts from `on_tick` use the owning plugin's authorization and root-context callbacks — see [Timers](#shared-data-and-timers).
- **Pause required**: request-header dispatch followed by Continue applies `failOpen` and discards the execution (`callout requires pause`). Tick callouts have no Pause action and are delivered after the hook returns.
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

Listing a Wasm entry under `upstreamResponseBodyFilterPlugins` invokes `proxy_on_response_body` once per upstream chunk. Request entries can also select `response.body: true` to observe chunks using their retained HTTP context. In both cases, the main upstream response remains streaming and has no whole-response buffer mode (see [Remaining limitations](#remaining-limitations)).

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

- Attached callbacks may retain bounded incremental state in the request guest object (for example a running digest or byte count), never an unbounded full response. Standalone body-filter entries use independent per-hook contexts and provide no request-state continuity. Shared counters are plugin-generation state, not a substitute for per-request isolation.
- **Strictly read-only**: body callbacks cannot modify bytes or short-circuit an already-started response. Attached body/trailer frames also reject logging and Pause; use bounded state for observation, not response enforcement.
- Each chunk runs under a reduced CPU budget (`timeoutMs / 10`, floor 10ms) so a slow guest cannot stall a long stream; a trap in the body hook **never interrupts the stream** (the instance is discarded and streaming continues, regardless of `failOpen`).
- A guest that does not export `proxy_on_response_body` is a free no-op at this stage.

## Shared data and timers

### Shared data

`proxy_get_shared_data` / `proxy_set_shared_data` back a plugin-runtime KV store with proxy-wasm CAS semantics (per-key monotonically increasing CAS token; `set` with a nonzero expected CAS fails with `CasMismatch` on mismatch; CAS 0 writes unconditionally; a null value deletes the key).

- **Scope: one configured plugin instance in one gateway process.** Its pooled VMs and tick share the store across requests. The common plugin layer reuses the same instance across references to the same entry in a configuration generation. Separate entries, separate stages, and rebuilt generations have independent stores, even when bytecode and configuration are identical. For cluster-wide coordination use `rate_limit_redis`.
- **Authorization isolation:** a tick uses only its owning plugin's allowlist. Other entries cannot read its shared data or grant it additional origins.
- Limits: 64KiB per value, 4MiB total per module. An oversized write returns `BAD_ARGUMENT`. The total is accounted as key + value bytes **plus a fixed ~128B per-entry overhead** (approximating real memory cost), so workloads with many tiny keys get proportionally less logical space — the cap bounds actual memory, not just payload bytes.
- Lifecycle: freed after the last holder of the loaded generation releases it, including outstanding request leases (see [Plugin runtime lifecycle](#plugin-runtime-lifecycle)). A rebuilt plugin or refreshed OCI module starts with an empty store; old and new stores may briefly coexist.

### Timers (`on_tick`)

A guest that calls `proxy_set_tick_period_milliseconds(N)` gets one background
task per loaded runtime generation (floor 100ms). It owns a dedicated persistent
VM under `timeoutMs`; consecutive successful ticks preserve its root fields.
A trap discards that VM, so a replacement starts with a new root. Request VMs
have separate roots: only explicit host shared KV crosses this boundary. A
root-field update in `on_tick` is not visible to request contexts. The guest can
change the period or stop ticking (`0`); plugin retirement or module replacement
cancels the task, with synchronous work bounded by its epoch deadline.

Typical pattern: `on_tick` dispatches a callout to fetch the latest ruleset and writes it into shared data; request hooks read it (rate-limit rule refresh, denylist sync, config polling).

#### Callouts from `on_tick`

`on_tick` may dispatch HTTP callouts like the request phase does. Differences:

- **No Pause/resume**: `proxy_on_tick` has no Action return; callouts queued during the hook are executed right after it returns, and each result is delivered via `proxy_on_http_call_response` on the **root context** (which lives as long as the instance — no per-request context is involved).
- **Plugin-local authorization.** Tick dispatch uses this plugin instance's `calloutAllowlist` and `calloutTimeoutMs`. An empty allowlist disables its tick callouts. Another plugin using the same bytecode neither grants permission nor creates a conflict.
- The timeout is capped by this plugin's `calloutTimeoutMs`; the other limits (8 dispatches per tick, 4 rounds, concurrency, and response-body cap) still apply.

## Failure semantics

Request and response-header execution failures use `failOpen`. Attached streaming
body/trailer failures disable the binding and record `WasmExecutionError` once,
while preserving the already-started response. Terminal cleanup failures also
record this bounded access-log diagnostic and discard the VM. They cannot
retroactively replace a response.

Pre-commit failure decisions:

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
- **Guest linear-memory cap** — guest memory growth is capped at **128 MiB** (and tables at 20,000 entries), with lower deployment limits available through `gateway.wasm_limits`. This also bounds the size of any single value the guest hands to a host call.

## Plugin runtime lifecycle

Each configured Wasm entry owns its loaded module, instance pool, shared data, and background work. References to the same plugin instance share that runtime through the common plugin layer.

- **Configuration replacement.** Rebuilding an entry creates a fresh runtime. Different entries and stages do not share mutable state, even with identical source/configuration. In-flight requests may complete on their captured old plugin generation.
- **Automatic cleanup.** Releasing the last plugin owner cancels preload, refresh, and tick work and drops its published module. Outstanding leases may retain the old module until they finish. There is no global runtime registry or 60-second module eviction grace; `edgion_wasm_module_released_total` counts released loaded generations.
- **Reload tradeoff.** Rebuilds may recompile and reset shared data. Only the process-wide engine and bounded OCI artifact-byte cache remain shared. Old and new generations may briefly overlap; state continuity across configuration replacement is not guaranteed.
- **File replacement.** Overwriting a `.wasm` file takes effect when the owning plugin is rebuilt and loads the file again. A bare overwrite does not replace an already-loaded runtime.
- **Preload and refresh.** Eager initialization starts configured ticks before the first request. Cold failures are negative-cached. OCI `Always` refresh keeps the last successful generation on failure; successful replacement cancels the old tick and starts the new generation.
- **Capacity.** `maxActiveRequests` bounds live request leases per configured runtime, including overlapping OCI module generations. The default is 32, range 1..=64. Long streams hold admission until terminal cleanup. The process-wide `gateway.wasm_limits.max_instances` (default 128) includes idle, active, dedicated timer, and retiring-generation VMs. Admission fails immediately through the existing fail mode; there is no waiting queue. Idle pools consume capacity, so this is neither a fairness guarantee nor a process RSS budget. `instancePoolSize` bounds per-plugin idle retention; the global idle count and guest-byte limits further restrict it.

## Metrics

Source-scoped metrics carry a `module` label; `edgion_wasm_modules_loaded` is process-wide. File sources use `file:<path>`, inline sources use `inline:<sha8>`, and URL sources use `url:<canonical-origin>#<path-sha8>`. A URL label is capped at 96 bytes, never contains userinfo, clear-text path, query, or fragment, and stays stable when only signed-query credentials change. The real URL remains part of the internal fetch and cache identity; it is not exposed through metrics or load-error logs.

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `edgion_wasm_module_load_total` | counter | `result=ok\|error` | Load attempts (negative-cache hits not counted) |
| `edgion_wasm_module_compile_duration_ms` | histogram | | `Module::new` wall time |
| `edgion_wasm_module_released_total` | counter | | Loaded runtime generations released after their last holder exits |
| `edgion_wasm_instance_instantiate_total` | counter | | Fresh instantiations (pool misses) |
| `edgion_wasm_instance_discard_total` | counter | `reason=poisoned\|over_capacity` | Poisoned or over-capacity instances discarded; normal dedicated timer retirement is not counted |
| `edgion_wasm_pool_size` | gauge | | Last reported pool size for the source label; not a sum across independent runtimes |
| `edgion_wasm_hook_duration_ms` | histogram | `phase` | Guest hook wall time (attached streaming observers use only request diagnostics; `request_headers`, `request_body`, `response_headers`, `response_body`, `http_call_response`, `tick`) |
| `edgion_wasm_trap_total` | counter | `phase` | Traps / protocol violations |
| `edgion_wasm_fail_mode_total` | counter | `reason`, `mode=open\|close` | Fail-mode decisions |
| `edgion_wasm_callout_total` | counter | `result=ok\|error\|denied\|over_limit` | Callout outcomes under the owning plugin's grant |
| `edgion_wasm_modules_loaded` | gauge | | Live compiled runtime generations, including initialization and outstanding old leases |
| `edgion_wasm_shared_data_bytes` | gauge | `module` | Sum of shared-data bytes across all live stores with the same module label, including overlapping generations and configuration variants; keys + values + ~128B/entry overhead. The 4MiB cap applies to each store, so the aggregate may exceed it. Returns to zero after the last store is released; the zero series may later expire under the configured idle timeout. |
| `edgion_wasm_body_pause_unsupported_total` | counter | `module` | **Alert on this.** A guest returned `Action::Pause` from the **response-body observation** hook — it wants whole-body buffering, which is not available on the response side (only on the request side, via [Request-body buffer mode](#request-body-buffer-mode)) — and is silently observing only the current chunk instead |

A trap storm shows up as `edgion_wasm_trap_total` and `edgion_wasm_instance_discard_total{reason="poisoned"}` rising together.

## `PluginLog` format

- `wasm ok ops=N logs=M` — success; `N` header operations applied, `M` guest log lines (content goes to the debug log).
- `wasm ok ops=N pause-ignored` — the guest returned `Pause` without dispatching a callout or sending a local response; the request continues.
- `wasm callouts=N leftover=M` — request phase resolved `N` callouts (`M` undelivered; any undelivered token discards the VM).
- **Pause required**: request-header dispatch followed by Continue applies `failOpen` and discards the execution (`callout requires pause`). Tick callouts have no Pause action and are delivered after the hook returns.
- `wasm local <status>` — the guest sent a local response.
- `wasm <reason>:fail-open` / `wasm <reason>:fail-close` — one of the failure reasons above.

## Remaining limitations

- **Request-body buffer mode is selected by the exported hook**: the shared runtime lazily captures the complete request behind the gateway physical cap. An optional `body:` block adds pre-execution conditions, a per-plugin `maxBodySize`, and `onReadFailure` handling; it is not the capture switch. See [Request-body buffer mode](#request-body-buffer-mode).
- **No response-body buffer mode**: the main upstream response remains streaming. Wasm response-body observation is per-chunk and read-only — no body rewrite, no short-circuit from the body phase, and `Action::Pause` cannot be honored — so a guest written to the standard whole-body buffering idiom sees only the current chunk. See the warning in [Response-body observation](#response-body-observation-read-only).
- **Request-body rewrite only**: `proxy_set_buffer_bytes(HttpRequestBody, ...)` is valid only during the whole request-body hook. Response-body writes remain `STATUS_UNIMPLEMENTED`; other phases return a defined ABI error.
- **Static mode selection, not dynamic Pause/Continue**: unlike Envoy, the body mode a plugin gets is fixed at module-load time by which hooks the compiled guest exports, not chosen per-request by the guest's `Action` return value. See [Two body modes, chosen statically](#two-body-modes-chosen-statically).
- **No gRPC callouts** (`proxy_dispatch_grpc_call`): calling it traps.
- **No callouts from the response/body phases** (sync stages), and request-phase dispatch without `Pause` fails through `failOpen` and discards the execution. Tick-phase callouts use their owning plugin's `calloutAllowlist` (see [Timers](#shared-data-and-timers)).
- **No shared queues, foreign functions, or guest-defined metrics**: calling them traps.
- **Explicit ownership profiles**: a request entry retains one VM and HTTP context through its attached response callbacks. Independently configured synchronous response/header/body entries still use separate per-hook contexts and independent runtimes. Neither profile promises root-field sharing between request and timer VMs. Request trailers, deferred completion (`on_done = false`), and `on_log` are not supported.
- **`source.oci` v1 constraints**: registries must present a publicly-verifiable TLS certificate (loopback registries may use plain HTTP for local testing); no registry mirrors. See [`source.oci` — pull from an OCI registry](#sourceoci--pull-from-an-oci-registry).

## Building a guest

Start with the pinned Rust SDK reference guests. A language having a Proxy-Wasm SDK does not guarantee that its compiler output or runtime semantics match Edgion. Validate the exact SDK, compiler and target combination against the named profile:

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

Check, in order: (1) the target origin is listed in `calloutAllowlist` — empty list disables callouts entirely; (2) for request-headers-phase dispatch, the guest returns `Action::Pause` — dispatch followed by `Continue` fails through `failOpen` (`callout requires pause` in the access log); (3) the dispatch happens in the request-headers or tick phase — request-body, response-headers, and response-body phases return `UNIMPLEMENTED`; (4) for tick-phase dispatch, the owning plugin itself must authorize the origin — another entry's allowlist does not apply.

### Q: My request is failing with an unexpected `500` instead of running `proxy_on_request_body`?

Confirm that the `Wasm` entry is under `requestPlugins` and the compiled guest actually exports `proxy_on_request_body`. Then check the gateway physical cap and request eligibility (gRPC and body-carrying upgrades are refused). If the entry has `body:`, also check its `conditions`, `maxBodySize`, and `onReadFailure`: fail-close limit/read failures reject, while fail-open policy failures skip the entire entry. Admission refusal and the gateway physical cap never fail open.

### Q: My guest module isn't taking effect on the response phase?

For stateful execution, configure one `requestPlugins` entry with explicit `response` hooks. For independent per-hook execution, use `upstreamResponseFilterPlugins` (headers) or `upstreamResponseBodyFilterPlugins` (read-only body). Neither is configured directly under `upstreamResponsePlugins`.

### Q: The controller rejected my EdgionPlugins resource?

Common causes: `source` has zero or more than one of `file`/`inline`/`url`/`oci` set; `source.url` without `sha256`; `sha256` isn't 64 hex chars; `source.inline` isn't valid base64 or decodes to more than 1MiB; `source.fetch` without `source.url`; `source.oci.image` isn't a valid `registry/repo:tag` or `registry/repo@sha256:<digest>` reference; `calloutAllowlist` entries that aren't clean `scheme://host[:port]` origins; `timeoutMs`/`calloutTimeoutMs` out of `1..=10000`; `instancePoolSize` or `maxActiveRequests` out of `1..=64`, or `response` hooks on a standalone response-stage entry.

### Q: I replaced the `.wasm` file on disk but the gateway still runs the old build?

After overwriting the file, update the owning `EdgionPlugins` resource to rebuild its plugin instance. The new runtime loads the file again; the old runtime is released after its last plugin reference and outstanding leases finish. A bare file overwrite does not replace an already-loaded runtime.

### Q: Can two requests share state?

Yes — via `proxy_{get,set}_shared_data`, which is shared across pooled VMs belonging to the same configured plugin instance **within one gateway process**. Separately configured entries or stages do not share this state. It is not replicated across gateway replicas; cluster-level coordination still belongs to `rate_limit_redis`.

## Related links

- [ExtProc Plugin](./ext-proc.md) — the gRPC push-model alternative (see [Callout vs ExtProc](#callout-vs-extproc))
- [Filters overview](../README.md)

## Response header policy

Response map operations and local reply headers follow this policy. Replace-all replaces editable fields while retaining protected fields; allowed duplicate `Set-Cookie` values remain separate. Rejected operations retain existing best-effort handling.

[Response header policy](../gateway-api/response-header-modifier.md#response-header-policy).


## Process resource admission and retirement

Configure process admission in local Gateway YAML. These are the defaults:

```yaml
gateway:
  wasm_limits:
    guest_memory_bytes: 536870912          # 512 MiB across all Stores
    max_instances: 128                    # active + idle + timer + retiring
    max_preparations: 4                   # acquisition and compilation
    max_instance_memory_bytes: 134217728   # 128 MiB per linear memory
    max_instance_table_elements: 20000
    max_idle_instances: 32
    max_idle_guest_memory_bytes: 134217728 # 128 MiB retained in idle pools
    max_host_call_bytes: 134217728         # per host-call allocation profile
    max_host_abi_bytes: 33554432           # per-phase retained ABI budget
    max_host_abi_ops: 200000
```

Changes require a Gateway restart. Unknown keys in the local `gateway` section
or `wasm_limits` fail configuration parsing; invalid limits fail startup. The
former standalone `wasm_guest_memory_bytes` key has been removed and is rejected.
There is no environment/CLI override or per-plugin bypass.

All limits are positive except the two idle limits: setting either to zero
**disables pooling**, even for guests without linear memory. `max_instances` is
at most 1,024 and `max_preparations` at most 32. Idle count cannot exceed instance
count, and idle bytes cannot exceed total guest bytes. Per-instance memory/table
and host ABI settings may only tighten the safety ceilings shown above. The
Engine pool uses the configured instance/table capacities, not hidden defaults.
Per-plugin `maxActiveRequests` and `instancePoolSize` further restrict admission.

Guest bytes account linear memory and pointer-sized table slots across plugins
and generations. Allocation is charged before growth. A refused guest growth
returns the normal Wasm failure result; failed initialization or exhausted
instance/preparation capacity follows existing load/admission failure and
`failOpen` policy. `failOpen: true` may bypass an optional plugin, but never
bypasses a resource limit. Use `failOpen: false` for mandatory enforcement.
Admission never creates an unbounded waiter queue.

All source types share preparation capacity before config decoding, fetching and
compilation. Blocking work and undelivered results retain their slot after caller
cancellation. Source-specific network limits still apply independently.

Healthy idle instances must fit both global idle limits in addition to their
plugin pool limit. Otherwise they are destroyed after callback payloads are
released. Checkout releases idle admission while retaining the instance and guest
charges. This limits pool retention across plugins; it is not active-plugin
fairness or pressure-driven eviction. Active requests and timer roots are never
reset to reclaim memory.

These are bounds on named resources, **not a process RSS limit**. Compiler heap,
compiled code, Engine virtual reservations/retained pages, runtime metadata,
GC heap, host snapshots, BodyStorage and transport buffers have separate owners.
The host ABI budgets bound their existing allocation profiles, not all host heap.
Shared Wasm memories remain unsupported.

For capacity planning, measure baseline Gateway memory and compilation peaks,
then leave room for artifact caches, transport/body storage and concurrent host
working sets in addition to guest bytes. Lower instance and preparation counts
for memory-constrained deployments; lower idle bytes/count to trade warm reuse
for less retained memory. A small guest-memory budget alone is insufficient.
For example, a stricter profile can set `guest_memory_bytes: 134217728`,
`max_instances: 16`, `max_preparations: 2`, `max_idle_instances: 4` and
`max_idle_guest_memory_bytes: 33554432`; this is not a claim that 128 MiB of
container memory is sufficient. Validate with representative plugins and traffic.

Admin `/api/v1/debug/store-stats` exposes `wasm_admission.limits`, `guest_bytes`,
`instances`, `preparations`, `idle_instances` and `idle_guest_bytes`. Counters are
independent samples; they are not a transactional or RSS measurement. Compare
usage against limits when tuning, including during configuration replacement.
Deployment/container memory limits remain the final process boundary; this
feature does not implement a Gateway-wide pressure-based overload manager.

Request callouts use the earlier of their configured timeout and the applicable
route request/gRPC deadline. The deadline includes network permit waiting and
is not restarted between serial callouts. An expired request does not receive
another guest callout callback. Response-header snapshots reject more than 1,024
headers or 64 KiB of raw name/value bytes before copying (lossy UTF-8 expansion
can use up to three times that payload size). These limits supplement the existing
256 KiB callout body cap and egress restrictions.

Replacing or destroying a runtime drains its idle pool immediately. Existing
requests retain their own generation until terminal cleanup, but cannot return
instances into the retired pool. No active request or timer root is reset merely
to reclaim a high memory watermark. Valid streaming responses gain no new short
lifetime timeout. This does not guarantee fair allocation among active plugins.
