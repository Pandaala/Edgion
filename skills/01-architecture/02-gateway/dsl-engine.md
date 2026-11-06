---
name: gateway-dsl-engine
description: DSL engine architecture — data flow, source layout, plugin entry points, async execution and termination, panic isolation, DslRuntime lifecycle. The architecture entry for DslPlugin maintainers; user-facing syntax / builtin reference lives in docs/.
---

# DSL engine architecture (DslPlugin)

> EdgionDSL is a lightweight request/response rule engine built on a custom bytecode VM, positioned for "zero runtime dependency, compile-time safety, sub-millisecond execution".
>
> This page is the architecture entry for engine maintainers. User-facing syntax, builtin function signatures, and runtime tuning live in `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/` — do not duplicate them here.

## 0. Design philosophy & scope boundary

EdgionDSL is **not** a general-purpose embedded language (it deliberately stops short of Lua/JS). It runs in-process, on shared Pingora worker threads, and its source or bytecode is shipped controller→gateway as configuration. Its value is *flexible orchestration with a provable resource ceiling*, not *arbitrary computation*. Keep this boundary when evaluating any new capability:

- **Orchestration logic** (read headers, branch on conditions, rewrite, compose/reuse predicates) → may live in the DSL, but prefer the lightest possible form.
- **Computation / capability** (new protocols, heavy algorithms, external integrations, perf-sensitive paths) → push down into Rust as a `BuiltinId` (the `CallBuiltin` seam), never as a language feature. Adding a builtin is cheaper, safer, and more testable than growing the language.
- **Decision rule**: ask "is X orchestration or computation?" Computation always becomes a Rust builtin.
- **User-defined functions**: at most a restricted **expression-function + compile-time inlining** form (no call frame, no recursion, no in-body `return`/statements, no bytecode-format change → existing step/loop/call/stack budgets and the validator still cover it). Full block-style functions with a runtime call stack are intentionally deferred — they shift isolation cost (recursion/stack bounds, frame accounting, bytecode compat) onto our own sandbox, which Lambda/Lua externalize to a mature runtime + MicroVM. Do not chase Lua-like ergonomics here.

## 1. Data flow

```text
DslConfig { source?, bytecode? }             # bytecode wins if both are present
    ├─ Controller validate_plugins
    │    → compile_and_validate → resolve_script → static validation
    │    → status diagnostics (no Gateway VM, no source-to-bytecode field rewrite)
    └─ resource serialization → config sync → Gateway compile_plugins
         → generic validation gate + DslPlugin::resolve_parts
         → compile_and_validate → resolve_script
              source: parser → compiler → Validator → CompiledScript
              bytecode: bounded decode → version check → Validator → CompiledScript
         → Gateway DslRuntime + VmLimits + Vm::new
         → cached plugin → per-request execute_safe → Vm.execute
```

The source and bytecode fields remain serializable configuration; validation does
not replace source with bytecode. Both binaries share runtime-independent static
checks, but only the Gateway initializes the VM and builtins. Every Gateway
constructor must validate independently: Controller acceptance cannot substitute
for `resolve_script` or the post-decode `Validator` check. Source parsing also runs
on the Gateway when source is the selected representation, during configuration
construction rather than request execution. `Vm::new` remains a separate fallible
runtime initialization boundary.

See [the revalidated trust-boundary ADR](../../04-review/dsl/intentional-design/dsl-design-decisions.md#1-bytecode-must-be-serialisable-across-processes).

## 2. Source layout

The language core (parser, AST, compiler, bytecode, errors, value, validator) lives in the standalone `edgion-dsl` crate, shared with the controller for admission-time validation via `DslConfig::compile_and_validate`. The runtime — VM execution, builtin dispatch, and plugin wiring — stays in the gateway. The static validation entry points are `edgion_dsl::resolve_script` and `edgion_dsl::check_redis_link_names`.

| Stage / role | File |
|---|---|
| Parser (nom combinators, parse budget 50_000, source ≤64 KB) | `edgion-dsl/src/parser/` |
| AST node types | `edgion-dsl/src/ast.rs` |
| Compiler (type check, regex pre-compile, constant pool, max regex pattern length 1024 B) | `edgion-dsl/src/compiler/` |
| Compile-time entry that ties parser + compiler together | `edgion-dsl/src/validator.rs` — `compile_dsl_source()` |
| Bytecode format + `BYTECODE_VERSION` + (de)serialization (gated by `MAX_DECODED_BYTECODE_LEN = 256 KB`) | `edgion-dsl/src/bytecode.rs` |
| Standalone validator (also re-runs on deserialized bytecode) | `edgion-dsl/src/validator.rs` — `Validator` |
| Config-independent compile/validate entry points | `edgion-dsl/src/script.rs` — `resolve_script()`, `check_redis_link_names()` |
| VM (`async fn execute`, panic-isolated `execute_safe`, `VmLimits`) | `edgion-gateway/src/plugins/http/dsl/lang/vm.rs` |
| Builtin implementations (`call_builtin`, `call_builtin_http`, `call_builtin_redis`) | `lang/builtins.rs` |
| Value enum (`Value::Str(Arc<str>)`, opaque `Value::Body`, `List`, `Map(Box<IndexMap<String, Value>>)`) | `edgion-dsl/src/value.rs` |
| Body runtime policy and execution-local scan account | `lang/body_ops.rs`, `lang/body_scan_budget.rs` |
| DSL-owned JSON/regex scanners and compiled selection plans | `lang/body_scanners.rs` |
| Generic body scanners, bounded text collection, and mutations | `edgion-gateway/src/request_body/body_scanners.rs`, `edgion-gateway/src/request_body/operation.rs` |
| Runtime errors | `edgion-dsl/src/error.rs` |
| Plugin entry (RequestFilter + async UpstreamResponse impls, config-to-plugin factory) | `plugin.rs` |
| Per-plugin external-call context (limits + redis link table) | `runtime.rs` — `DslRuntime` |
| Static config schema + `compile_and_validate()` wrapper | `config.rs` — `DslConfig` (`edgion-resources` crate) |
| Unit & integration tests | `tests/` |

## 3. Plugin entry points

`DslPlugin` implements both `RequestFilter` (`requestPlugins`, async) and `UpstreamResponse` (`upstreamResponsePlugins`, async, Pingora `response_filter` hook) — built via `from_config()` and `from_config_response()` respectively from the same `DslConfig` (see `plugin.rs::resolve_parts`). DSL is not valid in the synchronous `upstreamResponseFilterPlugins` stage; the shared stage table (`HTTP_PLUGIN_STAGE_EXCEPTIONS` in `edgion-resources/.../edgion_plugins/validate.rs`) rejects it there at configuration validation.

Both factories call `resolve_parts()`, which:

1. Calls `config.compile_and_validate()` (`edgion-resources`, backed by the `edgion-dsl` crate) — the single source of truth for the static validation pipeline, in order:
   - Validate the plugin name, require `source` or `bytecode`, resolve every
     runtime-limit default/range through `DslConfig::resolve_runtime_limits()`,
     and validate the error-policy status.
   - `edgion_dsl::resolve_script()` — prefers pre-compiled `bytecode` over inline `source`. Source is compiled via `compile_dsl_source()`. Bytecode is re-validated through `Validator` after `CompiledScript::deserialize_base64`.
   - Reject the unsupported proxied response-body API, then run
     `DslConfig::validate()` for LinkSys and Redis policy sanity.
   - `edgion_dsl::check_redis_link_names()` — a best-effort bytecode scan that rejects `redis.*` calls whose first-arg link name is not in `linkSysRefs`. False-positive / false-negative limitations are documented inline in `edgion-dsl/src/script.rs`; runtime `ok=false` is the fallback.
2. Reuses the validated, default-resolved values from
   `DslConfig::resolve_runtime_limits()` to build `VmLimits` and `DslRuntime`,
   then constructs the `Vm`.

A static request-stage reference to `req.body` registers the fixed fail-close
body gate so capture is available before VM execution. The VM receives only an
opaque `Value::Body` handle: byte length is metadata, predicates/hash/JSONPath
scan the shared `BodyStorage` incrementally, and only `read(offset, max_bytes)`
materializes a returned body string. Memory-backed, spilled, and mutation-segmented
storage therefore share one logical API. Response-stage DSL never starts late
capture and may read only the cache populated in the request stage.

Body mutation opcodes (`clear`, `append`, `prepend`, `remove`, `replace`) are
accepted only in `requestPlugins`; config validation rejects them in response
stages. Mutations are transactional, operate on raw byte ranges, and remain
bounded by the gateway physical body cap and mutation-segment limit.

The same `DslPlugin` instance is shared across requests through `PluginRuntime` — only `&self` accessors run on the hot path. Per-request mutable state lives in `Vm::execute`'s local `VmState`, not in `DslPlugin`.

### Body capability runtime

`BodyValue::Request` is a host-neutral capability tag. It carries no storage
pointer, snapshot, or cursor and preserves the 24-byte `Value` layout. Each
operation resolves the current request body; storage pins an immutable snapshot
for that operation. All aliases observe a subsequently published mutation on
their next operation. Explicit snapshot readers for outbound transfer and
mirroring remain frozen, even when an alias changes the logical body.

Gateway-local `DslBodyOps` borrows the session, execution-local
`BodyScanBudget`, and immutable runtime limits only for the operation. It owns
body validation, shared error mapping, and scan reservation/settlement. The VM
retains stack handling and operation-specific argument/constant checks.
Mutations go through `PluginSession` to retain stage and replay-owner checks;
the facade never stores another body or bypasses generation-checked publication.

`BodyScanBudget` is the one account shared by scans and `http.call(Body)` in
each VM execution. Generic scans reserve the remaining allowance and settle
fetched raw bytes. Uploads retain their exact reservation and cancellation-aware
settlement. Cached SHA-256 can report zero scanned bytes. A storage failure
without accounting retains the reservation; dropping an operation does not
automatically refund potentially still-running blocking work.

To extend Body behavior, implement an owned scanner using `RawBodyScanner` or
`TextBodyScanner` and invoke it through the runtime policy boundary. A parser
that needs `std::io::Read` (the `json_get`/`json_select` Body scanners)
implements `ReadBodyScanner` and runs through `DslBodyOps::scan_read_with`:
one admitted scan-lane job pulls snapshot chunks directly, with no extra
thread, channel, or per-chunk owned copy. Generic
storage traversal, lossy decoding, and range mechanics belong in `request_body`;
DSL values, compiled JSON paths, and regex plans belong in the DSL runtime.
Scanners moved to blocking workers must remain `Send + 'static` and cannot
borrow the session or VM. Reuse the existing bounded scan worker lane.

`read(offset, max_bytes)` uses the storage-aware `read_text_range` collector.
The requested raw `max_bytes` must fit `max_string_len` (default 8 KiB;
operator-configurable up to 1 MiB). Reject oversize arguments with
`StringTooLong` before scan reservation and range clipping; do not silently
clamp them even for short or empty bodies. Admitted ranges retain the generic
raw scan-budget preflight, EOF clipping, and invalid-offset behavior.
Overflow is a bounded scan outcome carrying `bytes_scanned`, so settlement
precedes the output `StringTooLong`. No raw full-range materialization occurs.
The no-argument whole-body method and its opcode are removed in bytecode
version 5, with no alias or implicit no-argument `read` replacement. Internal
storage materialization APIs remain separate from the script API.

### Compiler-local storage

Physical local slots are allocated independently of source names. Loop lowering
uses anonymous slots for its cached bound, iterable and counter; only authored
bindings enter lexical name resolution. Both forms contribute to `local_count`
and share the same compiler limit, with unchanged numeric bytecode and VM storage.

### Bounded XML values

`edgion-dsl::xml` owns immutable `Value::Xml` Arc nodes, allocation-free lexical
tokenization, namespace/character/structure validation, cached expanded subtree
summaries and bounded construction/serialization. XML uses the existing builtin
resolution, arity and independent bytecode validation seams, with no grammar
extension or workspace dependency edge. Prefixes normalize while expanded names,
ordinary attribute order and mixed/repeated child order remain semantic.
The private leaf decoder borrows unchanged token text and allocates decoded scratch
only when transformation is necessary. Returned nodes and namespace bindings still
own their Arc payloads; the existing per-character work and byte checks remain.

Gateway `lang/xml_builtins.rs` owns runtime policy and an owned raw scanner.
`DslBodyOps::parse_xml` checks current physical/source admission, reserves the
shared BodyScanBudget and moves the live ValueWorkBudget into the canonical
permit-limited blocking scan lane. The VM holds an exhausted placeholder until
the worker returns its account; it restores the account before interpreting nil
or error. The at-most-1-MiB parser input is transient derived-operation scratch,
not another cache/replay source. Bounded finish may complete after cancellation;
the permit and owned state stay with the worker until exit.

All XML producers precharge allocation/work; cached expanded summaries include
repeated handles to bound alias expansion. Xml equality is handle identity and
implicit string conversion is rejected. `XmlSetBody` triggers request capture
and mutation-stage validation even without ReqBody; serialization finishes under
both XML/output and physical limits before PluginSession::overwrite_request_body.
Bytes consumes the output String; string-returning paths charge their final Arc
copy separately. Existing replay/framing/retry/mirror owners remain canonical.
Malformed/unsupported XML is nil; Body XML limits/storage/work and all set_body
output/publication failures are fixed BodyAccess. Pure-string limits retain the
normal DSL policy. There is no response-body XML API or external XML resolution.
See [the regression contract](../../04-review/dsl/regression-guards/bounded-xml-primitives.md).

## 4. Async execution and termination

### Explicit termination decisions

`OpCode::ReturnTerminate` returns `PluginRunningResult::ErrResponse { status: 502,
body: None }` in both DSL entry points. This is a successful VM control-flow
decision, independent of `DslErrorPolicy`, and `execute_safe` logs `Deny 502`
once. The shared plugin runtime owns local response writing or response-stage
denial, including HTTP-to-gRPC mapping. DSL termination never relies on the
bare-`ErrTerminateRequest` bug fallback required by design rule F.

The opcode and bytecode format are unchanged: source and validated bytecode
both reach the same VM branch. For response ownership, header/body changes,
and verification limits, see [the termination regression guard](../../04-review/dsl/regression-guards/terminate-explicit-local-response.md).

### Async execution

`Vm::execute` is `async fn` (so it can await `http.call` and `redis.*` builtins). Both DSL entry points are async, so the VM is always awaited directly:

| Phase | Trait method | Execution |
|---|---|---|
| RequestFilter | `async fn run_request(...)` | direct `.await` on `execute_safe(...)` |
| UpstreamResponse | `async fn run_upstream_response(...)` | direct `.await` on `execute_safe(...)` |

There is no sync-to-async bridge. The former response entry point lived in the synchronous `UpstreamResponseFilter` stage and used `block_in_place(|| Handle::block_on(...))`; that panicked on the current-thread runtimes Pingora builds per worker when `server.work_stealing` is `false`, and pinned a worker thread for the duration of every `http.call` / `redis.*` call. Keep DSL out of synchronous stages; see [the regression guard](../../04-review/concurrency/regression-guards/dsl-response-stage-async-no-block-in-place.md).

Stage-order consequence: the async response chain runs in `response_filter`, after every synchronous `upstreamResponseFilterPlugins` entry at all levels. In the same chain, request-owned response companions (ExtProc, Guardrail) and the `spec.waf` response phase of a runtime run before that runtime's DSL entries, and inner-level companions run before outer-level DSL, so DSL can overwrite a companion's header edits. The late request-body view is released after the async chain (`pg_response_filter.rs`), so a response-stage DSL can still read a request body captured by `RequestBodyBuffer`.

`UpstreamResponseBodyFilter` has no DSL entry point on purpose — that phase fires once per chunk, and the body-phase hot-path rules in [`05-plugin-system.md`](05-plugin-system.md) (the "no PluginLog in body phase" constraint) forbid per-chunk allocation. Running the VM there would violate them outright.

## 5. Panic isolation

`vm::execute_safe` wraps `Vm::execute` in `std::panic::AssertUnwindSafe(...).catch_unwind().await`. If the future panics:

- The plugin returns the configured `errorPolicy` outcome (`Ignore` / `Deny` / `DenyWith`).
- The request is not aborted; subsequent plugins still run, subject to the same policy.
- The panic does not propagate into Pingora's worker — Pingora's request loop does not tolerate an unwinding panic on the hot path.

This pairs with the runtime limits in `VmLimits` (step / loop / call / stack / string / list / map): the limits are the first line of defence — they raise typed `RuntimeError`s through normal control flow — and `catch_unwind` only catches the "this should never happen" tail. Both layers must stay in place; removing the limits would shift defensive load onto `catch_unwind`, which is more expensive and harder to reason about.

`RuntimeError::BodyAccess` is intentionally handled before configurable
`errorPolicy`: storage, scan-budget, invalid-range, and mutation failures return
the fixed generic HTTP 500 body `Request body processing failed`. This is an
internal hard-failure classification, not an additional configurable policy
shape. Parse/type outcomes remain ordinary DSL semantics—for example,
`json_get()` returns `nil` for invalid JSON, missing paths, or type mismatch.

## 6. `DslRuntime` lifecycle

`DslRuntime` is **constructed per-plugin-instance at config-resolve time**, not per-request and not lazily — see `plugin.rs::resolve_parts`. It is `Send + Sync` and stored by value in `DslPlugin` (`self.runtime: DslRuntime`), so the hot path is a simple `&self.runtime` borrow.

```rust
// edgion-gateway/src/plugins/http/dsl/runtime.rs
pub struct DslRuntime {
    pub max_timeout_ms: u64,            // http.call wall-clock cap
    pub max_request_body_bytes: usize,  // opaque request Body cap
    pub max_response_body_bytes: usize, // http.call body cap
    pub block_private_ips: bool,        // http.call literal and resolved-address guard
    pub redis_links: HashMap<String, String>, // alias → "namespace/name" for get_redis_client
    pub rate_limits: DslRateLimits,     // immutable named rate-limit policies + shared executor
    pub max_body_scan_passes: u8,       // cumulative body-scan budget in physical-cap passes
}
```

Shared resources are global, not per-`DslRuntime`:

- `dsl_http_client(block_private_ips, follow_redirects)` selects one of four process-global `OnceLock<reqwest::Client>` instances keyed by that boolean pair. Every client uses `pool_max_idle_per_host(4)`; replayable string/nil calls may follow at most ten redirects, while opaque Body calls use the no-redirect variants. The clients are built through the shared `build_outbound_client` factory with `disable_proxy: true`; per-call timeouts come from `DslRuntime`, not from the client.
- Redis clients are resolved per-call from `link_sys` via `get_redis_client()` and the alias map; this matches the `rate_limit_redis` plugin pattern (ArcSwap read).

Implication: if the plugin's config changes (e.g. `httpMaxTimeoutMs` is updated), the controller emits a new `EdgionPlugins` entry; the gateway rebuilds `PluginRuntime` during preparse and constructs a fresh `DslPlugin` — and therefore a fresh `DslRuntime`. There is no in-place mutation path.

## Cross-references

- User-facing syntax / builtin reference / sandbox-limits user view: `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/` (`README.md`, `language-syntax.md`, `api-reference.md`, `sandbox-security.md`, `bytecode.md`, `examples.md`)
- Maintainer view of the sandbox (where each cap is enforced, Point 02 Map/List caps, ReDoS posture, SSRF guard rationale): [`dsl-engine-security-model.md`](dsl-engine-security-model.md)
- Per-builtin implementation guides: `skills/03-coding/dsl/` (`http-call.md`, `redis-builtin.md`, `builtins-path.md`)
- `Value::Str` / `Value::List` design decision (why `Arc<str>`, not SSO): `skills/04-review/dsl/intentional-design/sso-migration-not-worth-it.md`
- Engine selection rationale (why custom VM vs Rhai / mlua / rune / Boa, with reverse index of the `dsl-vm-rhai-research` series): `skills/04-review/dsl/intentional-design/dsl-design-decisions.md`
- Surrounding plugin framework (4-phase model, conditional execution, body-phase constraints): `skills/01-architecture/02-gateway/05-plugin-system.md`
