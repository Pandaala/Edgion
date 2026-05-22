---
name: dsl-error-line-number-declined
description: Use when reviewing proposals to surface DSL runtime-error source line numbers into the access log (e.g. `push_err_at_line`, RuntimeFault wrapper). The proposal is declined; the access log keeps the kind-only format.
---

# DSL runtime-error line numbers in access log — DESIGN-TRADEOFF (DECLINED)

## Conclusion

Surfacing DSL runtime-error source line numbers into the access log (e.g. `plugin=my-dsl err: division by zero @ line 2`) is **not adopted**. The access log keeps the kind-only format produced by `PluginLog::push_err(kind: &'static str)` (see `vm.rs` `execute_safe` error-kind match around the `RuntimeError::*` arms).

Fix suggestions of the form "add `push_err_at_line(kind: &'static str, line: u32)` API plus `RuntimeFault { kind, at_line: Option<u32> }` wrapper plus `CompiledScript::source_map` RLE" are **not accepted**.

## Core Rationale

**1. The `push_err(&'static str)` invariant is the load-bearing security boundary, not a stylistic choice**

`skills/03-coding/01-log-safety.md` Iron rule 1 documents the design: `PluginLog::push_err` accepts only `&'static str` so that `format!(..)`, `to_string()`, and any local `&str` containing user input fail to compile at the call site. Most leakage paths (JWT error Display, reqwest URL fragments, claim values) are blocked at compile time without depending on review eyeballs. Any new "safe opening" past this gate has to maintain that property: a single trybuild coverage gap turns the API into an unguarded `format!` channel.

**2. The DSL line-number gain is marginal compared with the cost of the safe opening**

Realistic DSL scripts in production are <50 lines (sandbox bytecode validation rejects anything over `max_instructions = 1000` which translates to roughly 500 source lines). The `RuntimeError` variants already classify the failure into one of 13 kinds (`type error` / `division by zero` / `integer overflow` / `step limit exceeded` / `loop limit exceeded` / `call limit exceeded` / `stack overflow` / `string too long` / `list too long` / `map too large` / `undefined local` / `api error` / `regex error` / `internal error`). The combination kind + script source is already a near-deterministic locator; the line number saves at most ~30 s of operator grep per debug session.

**3. The implementation surface multiplies the risk**

Adding the line-number path requires: a new wrapper type `RuntimeFault` distinct from `RuntimeError` (the 13 variants stay so their `Display` impls do not gain user input), all ~44 fault returns in `vm.rs` rewritten through a `self.fault(state, kind)` helper, the upstream issues `issue-11` (parser Span propagation) and `issue-12` (`CompiledScript::source_map` RLE — about 3 days of work) shipped first or the new path is dead code, a new `push_err_at_line(kind: &'static str, line: u32)` in `runtime/log.rs`, and a trybuild compile-fail test guarding the API against `&str` / `&String` / `format!` regressions in perpetuity. Total realistic effort is on the order of a week, and every future change to the DSL fault path has to remember the trybuild guard.

## Fix Suggestions Not Accepted

- **"Add `push_err_at_line(kind: &'static str, line: u32)` API"** — opens a second `push_err` family that must be guarded by trybuild in perpetuity; one missing test lets `&str` through and the access-log token-leak guarantee is gone. The marginal usability gain is too small to justify owning that ongoing maintenance burden.
- **"Add `RuntimeFault { kind: RuntimeError, at_line: Option<u32> }` wrapper and rewrite all `vm.rs` faults through `self.fault(state, kind)`"** — ~44 call-site change for a feature whose only consumer is the declined access-log line. Without that consumer the wrapper is dead code; with it the wrapper has to coexist with two-mode access-log emission (`push_err` for v=1 legacy bytecode, `push_err_at_line` for new bytecode), doubling the test matrix.
- **"Just relax `push_err` to accept `&str`"** — explicitly forbidden. The `&'static str` is the cross-plugin safety invariant; loosening it for DSL alone reintroduces the leakage path for every other plugin (`forward_auth`, `oidc`, `hmac_auth`, ...) that already relies on the compile-time block.
- **"Use `push_err_sensitive` for line numbers"** — overkill: that API exists for genuinely dynamic, potentially-secret error detail with 64-byte truncation and sensitive masking; running it for a 1–10 character integer is the wrong tool, and it still doesn't keep the line semantically separate from the kind.
- **"Add the line to a separate column like `plugin_log.line` field"** — the access-log schema is shared across all plugins; adding a DSL-only column either requires every plugin to populate it (most can't) or accept a special-case field that complicates the existing access-log entry parser.

## Re-evaluation Triggers

Re-open this decision only if **any** of the following is observed:

- A concrete production incident where operators provably could not locate a DSL bug from kind + script source, AND the script was <500 lines (i.e., within the validator's instruction-budget bound).
- DSL scripts in production routinely grow past ~200 lines AND multi-call-site-per-kind patterns become common (e.g., five `/` operators on different lines all candidate for `division by zero`).
- A new "safe opening" design emerges that doesn't depend on a trybuild compile-fail guard (e.g., a Rust type-level mechanism that statically rejects non-literal `&str` at the type system layer without a separate test crate).
- The DSL gains a debugging UI / control-plane RPC that consumes line numbers from a side channel other than the access log (in which case `CompiledScript::source_map` from `issue-12` may be revisited on its own merits, decoupled from the access-log path).

## Reference Cases

- `tasks/todo/dsl-vm-rhai-research/issues/issue-13-runtime-error-line-in-access-log.md` — declined 2026-05-16.
- `tasks/todo/dsl-vm-rhai-research/03-source-position-errors.md` — parent report; §5.2.5 enumerates the constraints; §1.6 documents the user pain point.
- `skills/03-coding/01-log-safety.md` — Iron rule 1 (the `push_err(&'static str)` invariant) and `observability/00-access-log.md` (the API spec).
- `src/core/gateway/plugins/http/dsl/lang/vm.rs` — `execute_safe` error-kind match; `RuntimeError` variant set.
- `src/core/gateway/plugins/runtime/log.rs` — `PluginLog::push_err` signature and `push_err_sensitive` API.
- Related declined-but-distinct work: `issue-11` (parser Span propagation) and `issue-12` (`CompiledScript::source_map` RLE) lose their primary consumer with this decision but may still be useful for compile-time / validator error messages; evaluate independently.
