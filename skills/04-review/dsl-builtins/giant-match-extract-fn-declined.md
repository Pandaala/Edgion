---
name: giant-match-extract-fn-declined
description: Use when a reviewer flags `Vm::call_builtin` (~750-line match) or `Vm::call_builtin_redis` (~315-line match) in `lang/builtins.rs` as a readability/testability defect and proposes extracting each arm into a named per-builtin `fn builtin_xxx(...)`; covers why the wholesale extract-fn refactor is not accepted and what the legitimate narrow extractions look like.
---

# `call_builtin` Giant-Match Per-Arm Extract-Fn — DESIGN-TRADEOFF

## Conclusion

The monolithic shape of `Vm::call_builtin` (~750 lines, ~70 match arms — one combined OR-arm covers the 13 `Redis*` variants that are dispatched asynchronously, the rest are per-variant) and `Vm::call_builtin_redis` (~315 lines, dispatching the 13 `Redis*` variants) in `src/core/gateway/plugins/http/dsl/lang/builtins.rs` is intentional. Wholesale extraction of each match arm into a separate `fn builtin_xxx(&self, state, session, log, ...) -> Result<Value, RuntimeError>` is **not accepted**: the cost is a large hot-path reshuffle (~1050 LoC across the two functions, ~80 new four/five-parameter fn signatures), the "per-builtin unit-testability" benefit is largely illusory (the extracted fns still require mocking `&mut dyn PluginSession` + `&mut VmState` + `&mut PluginLog`, which is exactly the integration-test surface already covered under `tests/`), and the change interacts adversely with the non-exhaustive-dispatch protection documented in [[builtin-dispatch-non-exhaustive-lint]] (today a forgotten arm fails compile via E0004; after wholesale extraction a forgotten arm splits into "forgot the helper fn" AND "forgot the dispatcher delegation", **growing** the omission surface).

Fix suggestions of the form "extract every `BuiltinId::*` arm into its own named fn for testability/readability" are **not accepted**. Narrow extractions of pure side-effect-free logic (see "Accepted narrow extractions" below) remain on the table when a concrete need arises.

## Core Rationale

**1. The match is tight by construction, not by neglect**

The ~70 arms in `call_builtin` and 13 in `call_builtin_redis` almost all share the same shape: pop N args from `state`, call one `session.*` / `log.*` method, push the result through a length-checking helper (`checked_value_str`, `checked_value_opt_str`, `redis_ok`, `redis_err`). The helpers that *can* be shared already are (`redis_ok`, `redis_err`, `check_string_len`, `checked_value_str`, `checked_value_opt_str`, `api_error` at the top of the file). What remains in each arm is the per-builtin pop-pattern, the one session-API call, and the post-processing — collectively 3–15 lines. A "one fn per arm" refactor would lift those 3–15 lines into a fn body whose signature is **larger than the body**:

```rust
// hypothetical extracted form — signature bloat dominates the work
fn builtin_req_query(
    &self,
    state: &mut VmState,
    session: &mut dyn PluginSession,
) -> Result<Value, RuntimeError> {
    let name = state.pop()?.into_string();
    checked_value_opt_str(session.get_query_param(&name), self.limits.max_string_len)
}
```

Stamping this out ~70 times multiplies a 3-line body into a 9-line file-level item, and `call_builtin` becomes a flat dispatch table that already exists today as the match itself. The "named fn per arm" shape adds no information not already present in the `BuiltinId::*` variant name.

**2. The "per-builtin unit-testability" claim is overstated**

The issue's testability argument rests on "extract `IpInCidr` so it can be fuzzed in isolation". But the extracted fn's signature would still need `&mut VmState` (to pop the args) and `&mut dyn PluginSession` (it currently doesn't, but most siblings do, so the extraction is uniform only if all take the same args). A test harness that mocks those is not cheaper to build than the existing integration tests in `src/core/gateway/plugins/http/dsl/tests/` which already exercise every builtin through compiled DSL scripts. The narrowest legitimate win — making the CIDR bit-mask arithmetic unit-testable — does **not** require touching the dispatch shape:

```rust
// What's actually worth extracting (one helper, narrow, pure):
fn ip_in_cidr(ip: IpAddr, cidr: &str) -> Result<bool, &'static str> { /* ... */ }

// Then in the BuiltinId::IpInCidr arm:
let cidr = state.pop()?.into_string();
let ip = state.pop()?.into_string();
match (ip.parse::<IpAddr>(), ip_in_cidr(parsed_ip, &cidr)) { /* ... */ }
```

This is the "Accepted narrow extractions" path below — it captures the audit-value the issue is reaching for without the ~1050-line reshuffle.

**3. Wholesale extraction degrades the non-exhaustive-dispatch guarantee**

[[builtin-dispatch-non-exhaustive-lint]] documents that the load-bearing safety property on `BuiltinId` dispatch is the **absence of `_ =>` arms** in the four match sites, which makes rustc emit E0004 the moment a new variant is added. A wholesale extraction reshapes `call_builtin` into something like:

```rust
match id {
    BuiltinId::ReqHeader => self.builtin_req_header(state, session),
    BuiltinId::ReqMethod => self.builtin_req_method(state, session),
    // ... ~70 more lines ...
}
```

The dispatcher stays exhaustive, but a *second* failure mode appears: "the helper fn exists but was never wired into the dispatcher" (or vice versa). E0004 catches a missing dispatcher arm; it does not catch a leftover orphan helper or a typo'd delegation (`BuiltinId::ReqHeader => self.builtin_req_method(...)`). The current shape collapses both into one site — you cannot "forget to wire" what is inlined.

**4. Hot-path organizational change is a category we resist**

`Vm::call_builtin` is on the DSL plugin's per-request inner loop (one call per `Call` opcode per request). Rust will likely inline most extracted helpers, but "likely" is not "guaranteed", and the cost/benefit framing on a hot-path organizational refactor with **no functional, security, or measured-performance defect** is unfavorable: the issue itself self-classifies as "Readability and testability only; no runtime correctness or security defect at rest". Per the project's minimal-change posture (`CLAUDE.md` "Keep changes minimal — do not refactor or add features that were not requested"; cf. [[validate-refactor-value]] in memory: "verify real pain before expanding refactor scope"), a ~1050-line refactor needs a concrete pain signal — an actual builtin-addition that hit the size as a barrier, an audit miss traced back to the buried logic, profiling pointing at the match — none of which has surfaced for `call_builtin`.

## Fix Suggestions Not Accepted

- "Extract every `BuiltinId::*` arm into `fn builtin_xxx(...)` for testability" — body-vs-signature ratio inverts (rationale 1); proposed tests duplicate existing integration coverage (rationale 2); degrades dispatch-exhaustiveness guarantee (rationale 3); no concrete pain signal (rationale 4).
- "Group arms by category (req-read, req-mutate, resp-read, resp-mutate, crypto, string, collection, time, network, redis) into per-category dispatcher fns" — same fundamental drawback as per-arm: the category dispatchers themselves become 80–200-line matches that re-create the "giant match" complaint at a sub-scope, and the new layer adds a delegation hop without making any individual arm easier to read or test.
- "Apply the same refactor to `call_builtin_redis`" — same arguments; additionally, `call_builtin_redis`'s structure is already constrained by the explicit non-Redis-variant list documented in [[builtin-dispatch-non-exhaustive-lint]] (issue-14 fix), which a wholesale extraction would have to preserve and complicate.
- "Just extract `IpInCidr` plus wrap the rest in a uniform `fn builtin_xxx` shape so the file is consistent" — accepts the ~1050-line reshuffle to gain uniformity. See "Accepted narrow extractions" — narrow extraction of `IpInCidr`'s pure logic does **not** require any change to the surrounding dispatch shape.

## Accepted narrow extractions (when the trigger appears)

The following narrow extractions are pre-approved if a concrete need surfaces, and do **not** require a re-review of this rule:

- `fn ip_in_cidr(ip: IpAddr, cidr: &str) -> Result<bool, RuntimeError>` — pure CIDR bit-mask arithmetic, currently inlined in `BuiltinId::IpInCidr`. Trigger: a security audit of CIDR parsing or fuzz-coverage request.
- `fn header_names_to_value(names: impl Iterator<Item = String>, limit: usize) -> Result<Value, RuntimeError>` — shared between `ReqHeaderNames` and `RespHeaderNames`. Trigger: a third caller appears, or the 256-element cap diverges between request/response.
- `fn parse_redis_kv_args(...)` / similar — only if `redis.*` argument parsing diverges in a non-trivial way (e.g. a new SETEX-style multi-arg variant lands).

Each of these is single-purpose, single-caller (or 2–3 callers), and does not touch the dispatch shape. They are explicitly outside the scope of "the wholesale extraction is declined".

## Re-evaluation Triggers

Re-open the wholesale-extraction decision only if:

- A `BuiltinId` addition is ever blocked by the file's size (the contributor cannot find the insertion point, makes a wrong-arm placement, or the diff review can't be visually parsed). Track which specific addition hit the barrier.
- Profiling shows `Vm::call_builtin` dispatch as a measurable hot spot (e.g. `perf` flame graph shows ≥1% in the match arm scaffolding, distinct from the inner session-API calls).
- A security audit traces a real vulnerability (not a theoretical concern) to the burying of logic in the giant match — concretely, the audit must show that the burial caused or significantly delayed the discovery.
- The number of `BuiltinId` variants exceeds ~150 (currently 83 in `bytecode.rs`) such that the per-arm bodies start to drift from the 3–15 line norm and grow logic that is reused across multiple arms. The reuse, not the size, is the trigger — extract the reused pieces, not the dispatch shape.

## Reference Cases

- Original review finding: `tasks/plugins/plugins-http-dsl-F009-builtins-giant-match-arm-extract-fn.md` (closed via this entry, 2026-05-26).
- Sibling rule that depends on the dispatch shape: [`../architecture/builtin-dispatch-non-exhaustive-lint.md`](../architecture/builtin-dispatch-non-exhaustive-lint.md) — the "no `_ =>`" exhaustiveness guarantee on `call_builtin` / `call_builtin_redis`.
- Same-class refactor decisions held to the minimal-change posture: [`../architecture/plugin-runtime-god-match.md`](../architecture/plugin-runtime-god-match.md), [`../rate-limit/header-injection-duplication.md`](../rate-limit/header-injection-duplication.md) (rate-limit F-004 — divergent emitters left un-extracted by design).
- Source: `src/core/gateway/plugins/http/dsl/lang/builtins.rs` — `Vm::call_builtin` and `Vm::call_builtin_redis` (grep the fn names; line numbers drift); `BuiltinId` enum in `src/core/gateway/plugins/http/dsl/lang/bytecode.rs`.
