---
name: gateway-dsl-security-model
description: DSL sandbox security model — maintainer view of where each sandbox limit is enforced (file:line), Point 02 Map/List caps, ReDoS posture, SSRF guard rationale. User-facing limits and config knobs live in docs/.
---

# DSL sandbox security model (maintainer view)

> User-facing reference for what a script author sees lives in `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/sandbox-security.md`. This page is its maintenance dual: **where** each limit is enforced, **why** the current shape was chosen, and what to touch when adding a new cap or extending an existing one.

## 1. Layers of defense

The VM never trusts a single layer. Each layer is independent — a hole in one does not compromise the others.

The compile-time layers (parser, validator, compiler, bytecode loader) now live in the standalone `edgion-dsl` crate, shared by the controller and the gateway; the VM and builtins (runtime layers) remain gateway-only, since only the gateway constructs and executes a `Vm`.

| Layer | Where it runs | What it caps | Code |
|---|---|---|---|
| Parser | source → AST | source bytes, AST node count, parse-step budget | `edgion-dsl/src/parser/mod.rs:17,32-46,80-86,96-100` |
| Validator (post-decode) | bytecode → AST-shaped program | instruction count, local-var count, constant pool count & bytes, single-string-constant length, observed loop nest depth, `MakeMap(n)` operand, regex/DFA rebuild, definite JSONPath validation | `edgion-dsl/src/validator.rs` |
| Compiler | AST → bytecode | regex pattern length, literal-pattern enforcement, streaming-DFA validation, definite literal JSONPath enforcement | `edgion-dsl/src/compiler/` |
| Bytecode load | decoded base64 → struct | total decoded bytes ≤ 256 KB | `edgion-dsl/src/bytecode.rs:13,285-301` |
| VM (per-instruction) | runtime | steps, calls, stack depth, loops, string len, list len, map size | `lang/vm.rs:34-95,132-176` |
| VM init (regex precompile) | script load | per-pattern DFA memory ≤ 1 MiB | `lang/vm.rs:199-235` |
| Builtins (per-call) | runtime | per-builtin caps (split list size, range list size, header map size, http.call response size, etc.); shared `checked_value_str` / `checked_value_opt_str` helpers at `lang/builtins.rs:46-56` | `lang/builtins.rs:500-549,943-1208` |
| Body operation layer | runtime | cumulative scan bytes, physical body cap, bounded returned strings, mutation ranges and segment count | `lang/body_ops.rs`, `lang/body_scan_budget.rs`, `lang/body_scanners.rs`, `request_body/operation.rs` |
| Panic isolation | the whole `execute` | catches everything else | `lang/vm.rs` → `execute_safe` (`AssertUnwindSafe + catch_unwind`); see `dsl-engine.md` §5 |

**Gateway validation is independent of Controller acceptance.** Config sync may carry inline source, pre-compiled bytecode, or both serialized `DslConfig` fields (bytecode wins). Controller validation does not convert the resource to bytecode-only transport. Gateway `DslConfig::compile_and_validate` first applies the resource-owned runtime-limit range contract, then `edgion_dsl::resolve_script` parses/compiles/validates selected source, or decodes and re-validates selected bytecode through `Validator` before VM initialization. Hand-crafted bytecode or an out-of-range runtime scalar cannot bypass those checks by having passed Controller admission. `Vm::new` is a separate Gateway-owned initialization step, not a replacement for static validation. See [the current data flow](dsl-engine.md#1-data-flow).

## 2. Runtime limit enforcement — source map

These are the `VmLimits` fields and where every breach is raised. **When adding a new runtime limit, mirror this layout.**

| `VmLimits` field | Default | Where each breach is raised |
|---|---|---|
| `max_steps` | 10_000 | `vm.rs:168` `tick_step` (per instruction) |
| `max_call_count` | 500 | `vm.rs:176` `tick_call` (every builtin dispatch) |
| `max_loop_iterations` | 100 | `vm.rs` loop body counter (per `for` body) |
| `max_stack_depth` | 128 | `vm.rs` push paths (`StackOverflow`) |
| `max_string_len` | 8_192 | concat and regex results, every checked builtin return, and `http.call` response body plus each retained response-header name/value; HTTP response-limit breaches return the existing status-zero failure map |
| `max_list_len` | 1_024 | List construction lives entirely in builtins — `builtins.rs:510-549` (`split` pre-allocation estimate, `range` with `hard_cap = max_list_len.min(1000)`); also bounds `req.header_names` / `resp.header_names` via their own hard 256 cap (predates Point 02) |
| `max_map_size` | 512 | `vm.rs` `MakeMap`, `http.call` response-header retained-prefix traversal, `redis.hgetall` rejection, and the compile-time `MakeMap(n)` operand check in `edgion-dsl` |

`DslConfig::resolve_runtime_limits` owns the defaults and accepted ranges for
these fields: steps `1..=1_000_000`, loop iterations and calls
`1..=100_000`, stack depth `1..=1_024`, string bytes `1..=1_048_576`, List
elements `1..=16_384`, and Map entries `1..=4_096`. It also resolves the
non-VM scan and HTTP limits described below. Both Controller validation and
the standalone Gateway request/response factories call this contract; zero is
invalid for every limit.

Each entered loop emits one `LoopInit` and reaches exactly one matching
`LoopEnd` on natural exhaustion or `break`. The compiler patches `break` to
the `LoopEnd` opcode itself, not after it, so the VM pops the inner counter
before an enclosing loop executes `LoopBack`. A `while` `continue` targets its
`LoopBack`, while a `for` `continue` targets its increment step before
`LoopBack`; both paths count the iteration. At load time, validation rejects
ordinary backward jumps and jumps across lexical loop identities, including
between sibling loops at equal depth. `LoopBack` must point backward within
its own innermost loop. This preserves `maxLoopIterations` for accepted
bytecode; `maxSteps` remains an independent runtime bound.

Request-body scanning has a separate per-plugin runtime limit,
`DslRuntime::max_body_scan_passes` (default 4, configured as
`maxBodyScanPasses`, valid range 1–16). At request execution, the VM converts
this to `physical_body_cap × passes`, reserves before each potentially
full-body operation, and refunds unused bytes after an early match. Metadata
access such as `byte_len()` does not consume the scan budget. This is separate
from `VmLimits`: the absolute byte ceiling comes from the already-resolved
gateway request-body policy, not from bytecode. The execution-local
`BodyScanBudget` is shared with exact opaque-upload reservations. Scans charge
fetched raw chunks, not decoded bytes or the exact match position. A bounded
text-overflow result settles its fetched bytes before becoming `StringTooLong`;
storage failures without accounting retain the conservative reservation.

The outbound exchange limits are resolved by the same resource contract:
`httpMaxTimeoutMs` is `1..=60_000` and reuses the shared outbound timeout
ceiling; `httpMaxRequestBodyBytes` is `1..=32 MiB`; and
`httpMaxResponseBodyBytes` is `1..=1 MiB`. The response cap remains further
limited by `max_string_len`, and the opaque request Body remains further
limited by the gateway physical request-body cap.

Adding a new runtime limit requires:

1. Field, default, and range in the resource-owned
   `DslConfig::resolve_runtime_limits` contract, plus the consuming `VmLimits`
   or `DslRuntime` field.
2. Mirror the compile-time portion in `edgion-dsl/src/validator.rs` `Limits` if the bytecode can declare the value statically (`MakeMap(n)` is the model).
3. Raise the matching `RuntimeError` variant from every code path that can blow the cap.
4. Extend the user docs in `docs/en/.../dsl/sandbox-security.md` (and the zh-CN mirror).
5. Add a negative test in `tests/sandbox.rs` or a dedicated `tests/<limit>.rs`.

## 3. Compile-time limits — source map

| Constant / field | Default | Where |
|---|---|---|
| `MAX_SOURCE_LEN` | 64 KB | `edgion-dsl/src/parser/mod.rs:17` |
| Parse-step budget | 50_000 | `edgion-dsl/src/parser/mod.rs:26,41` (`parse_budget`) |
| `max_ast_nodes` | 2_000 | `edgion-dsl/src/parser/mod.rs:34,45` |
| `max_nesting_depth` (parser) | 64 | `edgion-dsl/src/parser/mod.rs:30,43` — parser stack-overflow guard, separate from the compiler's `max_loop_depth` |
| `max_instructions` | 1_000 | `edgion-dsl/src/validator.rs:17,35` |
| `max_locals` | 64 | `edgion-dsl/src/validator.rs:19,36` |
| `max_constants` | 256 | `edgion-dsl/src/validator.rs:21,37` |
| `max_string_const_len` | 4_096 B | `edgion-dsl/src/validator.rs:23,38` |
| `max_const_pool_bytes` | 32_768 B | `edgion-dsl/src/validator.rs:25,39` |
| `max_loop_depth` (observed) | 8 | `edgion-dsl/src/validator.rs:27,40,220-260` — recomputed from the bytecode, not trusted from the declared field |
| `MAX_REGEX_PATTERN_LEN` | 1_024 B | `edgion-dsl/src/compiler/mod.rs:25`, `edgion-dsl/src/compiler/emit.rs:79-86`, `edgion-dsl/src/validator.rs:11,106-113` (also re-checked when validating decoded bytecode, alongside a DFA-bounded rebuild at `:114-125`) |
| `MAX_DECODED_BYTECODE_LEN` | 256 KB | `edgion-dsl/src/bytecode.rs:13,297-301` |

These constants are deliberately **not** runtime-configurable. They protect the loader/parser themselves; exposing them to operator config would invite the bytecode-format attack surface to grow with deployment time.

## 4. Map and List size caps — Point 02 result

Before 2026-05-18, `max_list_len` and `max_map_size` existed only on paper for some callers and were not enforced consistently. Point 02 of the dsl-vm-rhai-research series traced six call sites that produced unbounded `Value::List` or `Value::Map` payloads and tied them all to a single pair of `VmLimits` fields:

| Site | Cap | Location |
|---|---|---|
| `Split(s, delim)` | `max_list_len` (estimated, before allocation) | `builtins.rs:500-518` |
| `Range(start, end)` | `min(max_list_len, 1000)` hard ceiling | `builtins.rs:519-549` |
| `MakeMap` opcode | `max_map_size` runtime + compile-time | `vm.rs:732`, `edgion-dsl/src/validator.rs:279-290` |
| `http.call` response headers | inspect a raw-header prefix until the retained Map is full; do not inspect the tail | `builtins.rs` response completion helper |
| `redis.hgetall` | reject above `max_map_size` | `builtins.rs:1204-1209` |
| `req.header_names` / `resp.header_names` | hard 256 (separate from `max_map_size`, predates Point 02) | see `docs/.../sandbox-security.md` "Per-Function Limit Summary" |

**Why two separate fields instead of one?** Maps and lists have very different growth patterns: lists accumulate via parser-driven splits/ranges with a known upper bound; maps grow from external sources (HTTP response headers, redis HGETALL replies) where the count is data-dependent. Sharing one cap would force operators to overshoot on one or the other.

**Why does `http.call` retain a header prefix while `redis.hgetall` rejects an
oversized map?** HTTP response headers are inbound third-party data we cannot
retransmit, so ignoring the uninspected tail keeps the script running. Redis HGETALL
operates on values the script explicitly requested; rejecting the oversized map is
the safer established behavior there.

The HTTP prefix rule is evaluated before inspecting each raw header. Non-UTF-8 values
are skipped without consuming a slot or causing their names to be checked. A duplicate
replaces an earlier value only when traversal reaches it before the retained Map is
full; a duplicate in the ignored tail is not examined. Every retained lowercase name
and UTF-8 value must fit `max_string_len`, or the complete call fails with its
status-zero result. An ignored tail is not examined even if it is oversized.

The compile-time `MakeMap(n)` operand check in `edgion-dsl/src/validator.rs:279-290` exists specifically to reject hand-crafted bytecode that ships `MakeMap(65535)` without ever having to allocate. Skipping the validator check would still be caught by the runtime check at `vm.rs:732`, but at the cost of one wasted allocation per request — undesirable on a hot path.

## 5. String length cap — what it does and doesn't cover

`max_string_len` caps **runtime-produced** strings — every concat,
`regex_find` / `regex_replace` result, checked builtin return, request-body
`read()` result, serialized `json_get()` result, and the live
aggregate string payload retained by `json_select()`. Body
predicates and `sha256()` scan storage without first constructing a complete
body string. It also caps the requested raw byte count of each `Body.read`
call (default 8 KiB; configured maximum 1 MiB). The facade rejects an oversized
argument with `StringTooLong` before scan reservation or range clipping,
regardless of the available body length. This does not cap the Body itself.

Body text reads select an immutable raw range and feed the existing
`LossyTextScanner` into `BoundedTextCollector`. Every append checks the exact
UTF-8 output-byte sum, including replacement characters and EOF carry, before
requesting capacity. Growth targets are geometric but capped at the string
limit; `try_reserve_exact` receives the target minus the current length.
Allocation failure is a body-processing failure, not an output-limit result.
On output overflow, the collector discards its prefix and stops, returning
accounting with `BoundedTextResult::OutputLimitExceeded`. The runtime reports
`limit + 1` as a lower-bound diagnostic rather than scanning the rejected tail.
An argument-limit diagnostic reports the requested raw length; the output
limit remains independent because invalid UTF-8 can expand during decoding.

This bounds retained text and requested output capacity. It does not promise
that total operation memory equals `max_string_len`: storage has an independent
bounded raw scratch buffer and bounded snapshot segment metadata, allocators
may round capacities, and conversion to `Arc<str>` may temporarily duplicate
the admitted text. There is still one canonical request-body cache.

`regex_replace()` uses the HTTP-plugin common bounded replacement helper. A
template containing no `$` stays on a literal `find_iter` branch; a template
containing `$` is compiled to regex 1.13 interpolation tokens and uses
`captures_iter`. Both branches check each logical output prefix before
requesting growth of the helper-owned output string. The no-match result does
not construct that output, so the VM can retain the original `Arc<str>`.
These are logical-output and requested-reservation bounds: regex matching may
use its own scratch space, an allocator may round a requested capacity, and
conversion of the finished `String` to `Arc<str>` is a separate allocation
boundary.

`http.call` is a separate plugin-initiated bounded exchange. Its effective response
body cap is `min(httpMaxResponseBodyBytes, max_string_len)`, enforced by the canonical
streaming helper. Declared or streamed overflow fails the whole call; exact-cap input
must reach EOF. Lossy UTF-8 conversion is allocated only after checked preflight proves
the expanded result fits. This does not permit aggregation of the main upstream
response and does not create another `BodyStorage`.

`redis.*` keeps its non-throwing result Map on dependency and input failures. Its
script-visible error is a fixed public class, never a dynamic dependency or
caller-controlled string. A class that does not fit `max_string_len` becomes the
generic one-byte class `e`; no sensitive prefix is truncated into the result.
Dynamic dependency detail is confined to the bounded sensitive PluginLog path.

`sha256()` uses one lazy `[u8; 32]` once-cell per immutable Body snapshot. The
first successful call scans the captured snapshot; concurrent and later calls
reuse that result and report zero scanned bytes. Initial capture publishes one
canonical snapshot, while every successful mutation publishes a distinct
snapshot with an empty digest cell. The cache never contains Body bytes,
errors, or keyed MAC state.

`md5()` and `hmac_sha256(key)` deliberately use the generic raw snapshot scanner
without caching. Each call consumes another body-scan pass. MD5 exists only for
compatibility protocols and is not a security digest. Body HMAC is a
MAC-generation API. It adds no persistent key or MAC cache and makes no extra
key String copy, but ordinary VM values may retain the caller's key and
returned hexadecimal MAC; memory zeroization is not promised. Ordinary DSL
equality is not constant-time, so scripts must not use it to authenticate an
attacker-supplied tag; authentication belongs in a dedicated verifier such as
the HMAC Auth plugin.

It deliberately does **not** cap string literals in the constant pool. Those are bounded at compile time by `max_string_const_len` (4 KB per literal) and `max_const_pool_bytes` (32 KB total). Mixing the two would force the operator to set `max_string_len ≥ 4 KB` to accommodate the largest legal literal, defeating the point of a per-request runtime cap.

## 6. Regex / ReDoS posture

All three regex operations (`.matches(pattern)`, `regex_find(s, pattern)`,
`regex_replace(s, pattern, replacement)`) require a **string-literal pattern**;
dynamic patterns are rejected before bytecode is emitted. `.matches()` stores
`Constant::StreamingRegex` and can operate incrementally on `String` or
`Body`; the find/replace functions remain string-only and store
`Constant::Regex`.

Patterns are **pre-compiled once per script**, not once per call:

- `Vm::new` rebuilds `Constant::Regex` into the bounded string regex cache and
  `Constant::StreamingRegex` into a forward dense DFA capped at 1 MiB.
- `OpCode::Matches` carries DFA state across body chunks, including the final
  end-of-input transition. `RegexFind` and `RegexReplace` use the string regex
  cache. No opcode pays a per-request compile cost.

Defense in depth around regex evaluation:

| Layer | Where | What it stops |
|---|---|---|
| String-literal requirement | `edgion-dsl/src/compiler/expr.rs:218-251` | Dynamic / attacker-controlled patterns |
| `MAX_REGEX_PATTERN_LEN = 1024 B` | `edgion-dsl/src/compiler/emit.rs:79-86`, `edgion-dsl/src/validator.rs:106-113` (re-checked after deserialization) | Pathologically long patterns |
| DFA + size limit during compile | compiler, validator, and `Vm::new` | Patterns that compile to multi-MB DFA tables |
| Body scan budget | `BodyScanBudget` via `DslBodyOps` | Repeated scans of adversarial or spilled input |
| `max_string_len` | `lang/vm.rs`, `plugins/http/common/bounded_regex_replacement.rs` | Oversized `regex_find` output and incremental `regex_replace` output construction |

The validator-side rebuild is the key safeguard against **hand-crafted
bytecode that ships a regex pattern the compiler would have rejected**. The
streaming DFA subset intentionally rejects unsupported Unicode-aware word
boundaries; an explicit ASCII boundary such as `(?-u:\b)` remains available.

## 7. Request-body JSON selection and mutation

`json_get()` embeds a `Constant::JsonPath`. The compiler and untrusted-bytecode
validator both require a definite path rooted at `$`, limited to 1,024 bytes
and 32 `.member`, `["quoted"]`, or non-negative `[index]` segments. Runtime
parsing validates the complete JSON document but materializes only the selected
value, bounded by `max_string_len`; invalid JSON, missing paths, and type
mismatch return `nil`.

`req.body.json_select()` embeds one self-contained `Constant::JsonSelect` with
1–64 literal `{name, path}` entries. Names are unique and at most 256 UTF-8
bytes; the configured `max_map_size` may lower the entry count. The validator
rechecks every path and constant-pool byte, then `Vm::new` builds one immutable
shared-prefix trie. Each execution performs one complete Body scan and JSON
parse, retains no full DOM, and returns a caller-ordered Map. Individual
selected strings, the live aggregate selected string payload, and each
canonical-object accumulator are bounded by `max_string_len`. The parser may
also own one in-progress candidate and a small constant number of encoding
copies; serde may temporarily own one decoded or escaped token bounded by the
physical Body cap. It never retains a complete Body DOM. Invalid JSON discards
every observed value and returns all `Nil`.

Body mutations are request-stage only and are transactional: an invalid range,
physical-cap breach, storage error, or 4,096-segment breach leaves the prior
logical body intact. Those failures, scan-budget exhaustion, and body-storage
failures become `RuntimeError::BodyAccess` and always use the fixed generic
HTTP 500 response. This internal classification must not acquire a
configuration knob or become a fifth failure-policy shape.

## 8. SSRF guard — literal precheck plus dial-time resolution

When `httpBlockPrivateIps: true` is set on the plugin config, `http.call` rejects:
- IPv4 loopback, private (10/8, 172.16/12, 192.168/16), link-local (169.254/16), unspecified (0.0.0.0)
- IPv6 loopback (`::1`), unique local (`fc00::/7`), link-local (`fe80::/10`), and `::ffff:`-mapped IPv4 forms of the above

Direct IP literals are rejected before a connection is attempted. Hostnames are
resolved through the canonical `EgressResolver`, which classifies the exact addresses
returned to reqwest for dialing. reqwest does not independently re-resolve those names,
so the check has no DNS-rebinding TOCTOU window. The DSL resolver uses metric-only
rejection reporting because its destination is request-derived; emitting one WARN per
blocked address would allow log amplification.

**Why is the default `false`?** Two reasons:
1. Many real deployments call internal services (Redis admin APIs, sidecar `/healthz` endpoints) from DSL scripts. Defaulting to "block private" would break the common case.
2. SSRF is a deployment-level concern: when the gateway runs in a cluster where `http.call` should never touch the cluster network, the operator enables the guard; when it runs as a per-tenant Lua-style scripting hook, the guard is on by default in the example configs (`edgion-tests/integration/conf/EdgionPlugins/Dsl/`).

**Redirect hops are guarded.** `Str` and `Nil` calls re-apply the literal precheck
and use the guarded resolver for each connection, for at most ten redirects. Opaque
`Body` calls do not follow redirects because their pull stream is single-use; the 3xx
response is returned directly to the script.

## Cross-references

- User-view sandbox doc: `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/sandbox-security.md` (and the zh-CN mirror)
- Engine architecture (data flow, source layout, `DslRuntime` lifecycle, panic isolation): `dsl-engine.md`
- `Value::Str` / `Value::List` design decision (why `Arc<str>`, not SSO): `skills/04-review/dsl/intentional-design/sso-migration-not-worth-it.md`
- `Value::Map` boxing rationale (24 B `Value` invariant): `skills/04-review/dsl/intentional-design/map-variant-is-boxed.md`
- Per-builtin implementation guides (where each cap is wired in): `skills/03-coding/dsl/` (`http-call.md`, `redis-builtin.md`, `builtins-path.md`)
- Plugin lifecycle (how `DslPlugin` reaches `Vm::execute`): `skills/01-architecture/02-gateway/05-plugin-system.md`
