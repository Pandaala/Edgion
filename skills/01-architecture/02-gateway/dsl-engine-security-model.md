---
name: gateway-dsl-security-model
description: DSL sandbox security model — maintainer view of where each sandbox limit is enforced (file:line), Point 02 Map/List caps, ReDoS posture, SSRF guard rationale. User-facing limits and config knobs live in docs/.
---

# DSL sandbox security model (maintainer view)

> User-facing reference for what a script author sees lives in `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/sandbox-security.md`. This page is its maintenance dual: **where** each limit is enforced, **why** the current shape was chosen, and what to touch when adding a new cap or extending an existing one.

## 1. Layers of defense

The VM never trusts a single layer. Each layer is independent — a hole in one does not compromise the others.

| Layer | Where it runs | What it caps | Code |
|---|---|---|---|
| Parser | source → AST | source bytes, AST node count, parse-step budget | `lang/parser/mod.rs:17,32-46,80-86,96-100` |
| Validator (post-decode) | bytecode → AST-shaped program | instruction count, local-var count, constant pool count & bytes, single-string-constant length, observed loop nest depth, `MakeMap(n)` operand, regex pattern length + DFA-bounded rebuild | `lang/validator.rs:11-46,61-128,210-290` |
| Compiler | AST → bytecode | regex pattern length, "pattern must be a string literal" enforcement (applies to `.matches()`, `regex_find`, `regex_replace`) | `lang/compiler/mod.rs:25`, `lang/compiler/emit.rs:79-86`, `lang/compiler/expr.rs:218-251` |
| Bytecode load | decoded base64 → struct | total decoded bytes ≤ 256 KB | `lang/bytecode.rs:13,285-301` |
| VM (per-instruction) | runtime | steps, calls, stack depth, loops, string len, list len, map size | `lang/vm.rs:34-95,132-176` |
| VM init (regex precompile) | script load | per-pattern DFA memory ≤ 1 MiB | `lang/vm.rs:199-235` |
| Builtins (per-call) | runtime | per-builtin caps (split list size, range list size, header map size, http.call response size, etc.); shared `checked_value_str` / `checked_value_opt_str` helpers at `lang/builtins.rs:46-56` | `lang/builtins.rs:500-549,943-1208` |
| Panic isolation | the whole `execute` | catches everything else | `lang/vm.rs` → `execute_safe` (`AssertUnwindSafe + catch_unwind`); see `dsl-engine.md` §5 |

**Pre-compiled bytecode shipped from the controller is re-validated by `Validator::validate` on the gateway side.** The controller's compile output is never trusted blindly — see `DslPlugin::resolve_script` for the order `deserialize → Validator::validate → use`. Hand-crafted bytecode payloads that declare `max_loop_depth: 0` while shipping deeper loops, or `MakeMap(65535)` operands, are rejected here (`validator.rs:220-290`).

## 2. Runtime limit enforcement — source map

These are the `VmLimits` fields and where every breach is raised. **When adding a new runtime limit, mirror this layout.**

| `VmLimits` field | Default | Where each breach is raised |
|---|---|---|
| `max_steps` | 10_000 | `vm.rs:168` `tick_step` (per instruction) |
| `max_call_count` | 500 | `vm.rs:176` `tick_call` (every builtin dispatch) |
| `max_loop_iterations` | 100 | `vm.rs` loop body counter (per `for` body) |
| `max_stack_depth` | 128 | `vm.rs` push paths (`StackOverflow`) |
| `max_string_len` | 8_192 | `vm.rs:327,341,355` (concat branches), `vm.rs:593` (`regex_find` result), `vm.rs:626` (`regex_replace` result), plus every checked builtin return via `checked_value_str` / `checked_value_opt_str` (defined at `builtins.rs:46-56`, used throughout) |
| `max_list_len` | 1_024 | List construction lives entirely in builtins — `builtins.rs:510-549` (`split` pre-allocation estimate, `range` with `hard_cap = max_list_len.min(1000)`); also bounds `req.header_names` / `resp.header_names` via their own hard 256 cap (predates Point 02) |
| `max_map_size` | 512 | `vm.rs:732` `MakeMap`, `builtins.rs:943,1204` (`http.call` response headers truncate at this cap; `redis.hgetall` rejects above it), `validator.rs:279-290` compile-time `MakeMap(n)` operand check |

Adding a new runtime limit requires:
1. Field + default in `VmLimits` (`vm.rs:34-95`).
2. Mirror the compile-time portion in `validator.rs` `Limits` if the bytecode can declare the value statically (`MakeMap(n)` is the model).
3. Raise the matching `RuntimeError` variant from every code path that can blow the cap.
4. Extend the user docs in `docs/en/.../dsl/sandbox-security.md` (and the zh-CN mirror).
5. Add a negative test in `tests/sandbox.rs` or a dedicated `tests/<limit>.rs`.

## 3. Compile-time limits — source map

| Constant / field | Default | Where |
|---|---|---|
| `MAX_SOURCE_LEN` | 64 KB | `lang/parser/mod.rs:17` |
| Parse-step budget | 50_000 | `lang/parser/mod.rs:26,41` (`parse_budget`) |
| `max_ast_nodes` | 2_000 | `lang/parser/mod.rs:34,45` |
| `max_nesting_depth` (parser) | 64 | `lang/parser/mod.rs:30,43` — parser stack-overflow guard, separate from the compiler's `max_loop_depth` |
| `max_instructions` | 1_000 | `lang/validator.rs:17,35` |
| `max_locals` | 64 | `lang/validator.rs:19,36` |
| `max_constants` | 256 | `lang/validator.rs:21,37` |
| `max_string_const_len` | 4_096 B | `lang/validator.rs:23,38` |
| `max_const_pool_bytes` | 32_768 B | `lang/validator.rs:25,39` |
| `max_loop_depth` (observed) | 8 | `lang/validator.rs:27,40,220-260` — recomputed from the bytecode, not trusted from the declared field |
| `MAX_REGEX_PATTERN_LEN` | 1_024 B | `lang/compiler/mod.rs:25`, `lang/compiler/emit.rs:79-86`, `lang/validator.rs:11,106-113` (also re-checked when validating decoded bytecode, alongside a DFA-bounded rebuild at `:114-125`) |
| `MAX_DECODED_BYTECODE_LEN` | 256 KB | `lang/bytecode.rs:13,297-301` |

These constants are deliberately **not** runtime-configurable. They protect the loader/parser themselves; exposing them to operator config would invite the bytecode-format attack surface to grow with deployment time.

## 4. Map and List size caps — Point 02 result

Before 2026-05-18, `max_list_len` and `max_map_size` existed only on paper for some callers and were not enforced consistently. Point 02 of the dsl-vm-rhai-research series traced six call sites that produced unbounded `Value::List` or `Value::Map` payloads and tied them all to a single pair of `VmLimits` fields:

| Site | Cap | Location |
|---|---|---|
| `Split(s, delim)` | `max_list_len` (estimated, before allocation) | `builtins.rs:500-518` |
| `Range(start, end)` | `min(max_list_len, 1000)` hard ceiling | `builtins.rs:519-549` |
| `MakeMap` opcode | `max_map_size` runtime + compile-time | `vm.rs:732`, `validator.rs:279-290` |
| `http.call` response headers | silently truncate at `max_map_size` | `builtins.rs:938-947` |
| `redis.hgetall` | reject above `max_map_size` | `builtins.rs:1204-1209` |
| `req.header_names` / `resp.header_names` | hard 256 (separate from `max_map_size`, predates Point 02) | see `docs/.../sandbox-security.md` "Per-Function Limit Summary" |

**Why two separate fields instead of one?** Maps and lists have very different growth patterns: lists accumulate via parser-driven splits/ranges with a known upper bound; maps grow from external sources (HTTP response headers, redis HGETALL replies) where the count is data-dependent. Sharing one cap would force operators to overshoot on one or the other.

**Why `http.call` truncates while `redis.hgetall` rejects?** HTTP response headers are inbound third-party data we cannot retransmit — silently dropping the tail is the only way to keep the script running. Redis HGETALL operates on values the script asked for; failing loud is the safer default there.

The compile-time `MakeMap(n)` operand check in `validator.rs:279-290` exists specifically to reject hand-crafted bytecode that ships `MakeMap(65535)` without ever having to allocate. Skipping the validator check would still be caught by the runtime check at `vm.rs:732`, but at the cost of one wasted allocation per request — undesirable on a hot path.

## 5. String length cap — what it does and doesn't cover

`max_string_len` caps **runtime-produced** strings only — every concat (`vm.rs:327,341,355`), `regex_find` result (`vm.rs:593`), `regex_replace` result (`vm.rs:626`), and every checked builtin return (`checked_value_str` / `checked_value_opt_str` at `builtins.rs:46-56`).

It deliberately does **not** cap string literals in the constant pool. Those are bounded at compile time by `max_string_const_len` (4 KB per literal) and `max_const_pool_bytes` (32 KB total). Mixing the two would force the operator to set `max_string_len ≥ 4 KB` to accommodate the largest legal literal, defeating the point of a per-request runtime cap.

## 6. Regex / ReDoS posture

All three regex opcodes (`.matches(pattern)`, `regex_find(s, pattern)`, `regex_replace(s, pattern, replacement)`) require a **string-literal pattern**, enforced at the compiler in `lang/compiler/expr.rs:195-208` (`.matches()`) and `:218-251` (`regex_find` / `regex_replace`) — dynamic patterns are rejected before bytecode is emitted. The literal is stored as a `Constant::Regex(String)` (`bytecode.rs:247`) and never appears in a variable slot.

Patterns are **pre-compiled once per script**, not once per call:

- `Vm::new` (`vm.rs:199-235`) iterates `script.constants` and, for every `Constant::Regex`, builds a `regex::Regex` with `size_limit(1 << 20)` and `dfa_size_limit(1 << 20)`. The compiled regex is stored in `regex_cache: Vec<Option<Regex>>` indexed by constant id.
- `OpCode::Matches(regex_idx)` (`vm.rs:566-580`), `OpCode::RegexFind(regex_idx)` (`vm.rs:581-607`), and `OpCode::RegexReplace(regex_idx)` (`vm.rs:608-633`) all dispatch identically: `self.regex_cache.get(regex_idx as usize)`. No opcode pays a per-call compile cost.

Defense in depth around regex evaluation:

| Layer | Where | What it stops |
|---|---|---|
| String-literal requirement | `lang/compiler/expr.rs:218-251` | Dynamic / attacker-controlled patterns |
| `MAX_REGEX_PATTERN_LEN = 1024 B` | `lang/compiler/emit.rs:79-86`, `lang/validator.rs:106-113` (re-checked after deserialization) | Pathologically long patterns |
| DFA + size limit during compile | `lang/compiler/emit.rs:89-94`, `lang/validator.rs:114-125`, `lang/vm.rs:209-215` | Patterns that compile to multi-MB DFA tables |
| `max_steps` | `lang/vm.rs:168` | Pathological matching time on adversarial input |
| `max_string_len` | `lang/vm.rs:593,626` | Oversized `regex_find` / `regex_replace` output |

The validator-side rebuild at `validator.rs:114-125` is the key safeguard against **hand-crafted bytecode that ships a regex pattern the compiler would have rejected**: even if the controller's compiler is bypassed, the validator runs the same `RegexBuilder` with the same memory caps before the VM is constructed.

## 7. SSRF guard — `is_ssrf_blocked_ip_literal`

`http.call` resolves the URL host and, when `httpBlockPrivateIps: true` is set on the plugin config, rejects:
- IPv4 loopback, private (10/8, 172.16/12, 192.168/16), link-local (169.254/16), unspecified (0.0.0.0)
- IPv6 loopback (`::1`), unique local (`fc00::/7`), link-local (`fe80::/10`), and `::ffff:`-mapped IPv4 forms of the above
- Implementation: `builtins.rs:1329-1357` (`is_ssrf_blocked_ip_literal` + `is_private_v4`). Tests live alongside in `#[cfg(test)] mod ip_tests` at `builtins.rs:1359-1430`.

**Why is the default `false`?** Two reasons:
1. Many real deployments call internal services (Redis admin APIs, sidecar `/healthz` endpoints) from DSL scripts. Defaulting to "block private" would break the common case.
2. SSRF is a deployment-level concern: when the gateway runs in a cluster where `http.call` should never touch the cluster network, the operator enables the guard; when it runs as a per-tenant Lua-style scripting hook, the guard is on by default in the example configs (`edgion-tests/integration/conf/EdgionPlugins/Dsl/`).

The blocklist is **host-string** based, not post-DNS-resolution. If the URL host is a name (`internal-svc.local`), the script can still reach private IPs by resolving in the upstream DNS. A future tightening would resolve before checking — out of scope today because it would require a synchronous resolver in the path. Reopen when an SSRF report cites this gap.

**Redirect hops are guarded (F-012, 2026-05-26).** With `httpBlockPrivateIps: true`, `runtime::dsl_http_client(true)` returns a separate process-global `reqwest::Client` whose `redirect::Policy::custom` re-applies `is_ssrf_blocked_ip_literal` to **every** redirect hop's URL host; a 30x to a blocked IP literal terminates the request (`status = 0`) instead of being silently followed. The same host-string-only limitation applies to redirect hops too — a redirect `Location:` pointing at a DNS name that resolves privately still goes through, by the same accepted tradeoff above.

## Cross-references

- User-view sandbox doc: `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/sandbox-security.md` (and the zh-CN mirror)
- Engine architecture (data flow, source layout, `DslRuntime` lifecycle, panic isolation): `dsl-engine.md`
- `Value::Str` / `Value::List` design decision (why `Arc<str>`, not SSO): `skills/04-review/dsl-value-str/sso-migration-not-worth-it.md`
- `Value::Map` boxing rationale (24 B `Value` invariant): `skills/04-review/dsl-value-str/map-variant-is-boxed.md`
- Per-builtin implementation guides (where each cap is wired in): `skills/03-coding/dsl/` (`http-call.md`, `redis-builtin.md`, `builtins-path.md`)
- Plugin lifecycle (how `DslPlugin` reaches `Vm::execute`): `skills/01-architecture/02-gateway/05-plugin-system.md`
