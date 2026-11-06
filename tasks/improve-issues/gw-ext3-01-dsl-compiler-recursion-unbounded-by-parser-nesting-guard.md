# gw-ext3-01: DSL compiler recursion is not bounded by the parser nesting guard; one CR line uses ~1.4 MiB of a 2 MiB worker stack

- **Category**: design-error
- **Severity**: P2 (worth doing)
- **Scope**: local
- **Location**: `edgion-dsl/src/parser/mod.rs:43,45` (`max_nesting_depth: 64`, `max_ast_nodes: 2_000`), `edgion-dsl/src/parser/expr.rs:30-104` (`parse_expr_bp`: postfix `.`/`[` and binary-operator loops never call `with_nesting`), `edgion-dsl/src/compiler/expr.rs:11-150,153` (`compile_expr` / `compile_method_call` recurse once per AST level), `edgion-dsl/src/validator.rs:537-552` (`compile_dsl_source`: parse -> compile -> validate, instruction limit checked only after compile), `edgion-resources/src/resources/edgion_plugins/plugin_configs/dsl.rs:369-394` (`compile_and_validate`, called by Controller ingest and Gateway `DslPlugin` construction)

## Summary
The parser documents "Nesting depth limit (prevents stack overflow)", but it counts nesting only for
parentheses, unary operators, call argument lists, map literals and index keys. Postfix member/method
chains (`x.len.len...`), index chains (`x[0][0]...`) and same-precedence binary chains (`x+x+x...`)
are built by loops in `parse_expr_bp`, so they produce a left-deep AST whose depth is limited only by
the 2,000-node budget. The compiler then recurses once (two frames) per AST level, and the AST drop is
recursive too. A single source line therefore needs roughly 1.3-1.5 MiB of native stack before the
1,000-instruction validator limit can reject it. A stack overflow aborts the process; it cannot be
caught by `catch_unwind` or turned into a status condition.

## Current behavior (evidence)
- `parse_expr_bp` (`parser/expr.rs:30-104`) handles `.` via `parse_dot_access` and `[` inline,
  then `continue`s the loop, wrapping the *previous* `lhs` in a new `Box<Expr>`. Only the index *key*
  is parsed under `with_nesting`; the chain itself is never counted. Left-associative binary operators
  likewise loop (`bp <= min_bp` breaks only between precedence levels), producing a left-deep tree.
- `compile_expr` handles `FieldAccess` by calling `compile_method_call`, which compiles `object`
  first (`compile_expr(object)`), and `BinaryOp` / `Index` compile `left` / `object` first. Recursion
  depth equals chain length. No depth counter exists in the compiler.
- `compile_dsl_source` runs the compiler before `Validator::validate`, so the `max_instructions`
  (1,000) check cannot prevent the deep recursion.
- Measured with a release build of `edgion-dsl` (probe crate outside the repository, calling the
  public `compile_dsl_source` on a dedicated thread): source `let x = "a"\nlet y = x` followed by
  1,996 `.len` segments (within the 2,000-node budget) overflows the stack and aborts the process
  with a 1.0, 1.125 or 1.25 MiB thread stack, and completes (rejected by "too many instructions")
  with 1.5 MiB. Tokio and Pingora worker threads use the 2 MiB default; no `thread_stack_size` is
  configured anywhere in `edgion-controller/src`, `edgion-gateway/src` or `edgion-common/src`. The
  remaining head-room for the caller's own frames (resource processor, conf_sync apply, plugin
  factory) is therefore about 0.5 MiB, and debug builds use substantially larger frames.
- The same source is compiled by the Controller (`validate.rs:633`, `edgion_plugin.rs:335`) and by
  every Gateway at plugin construction (`dsl/plugin.rs:53`). A crash in the Controller occurs on
  ingest; a crash in the Gateway repeats on every restart while the resource remains published.

## Industry comparison
- Lua (used by Kong, APISIX, Envoy Lua) bounds the C stack used by its recursive-descent parser with
  `LUAI_MAXCCALLS` (200), which counts every nested sub-expression level including chained suffixes,
  and reports "chunk has too many syntax levels" as an ordinary error.
- CEL (Envoy RBAC, Gateway API, Kubernetes validation) enforces a parser recursion limit
  (`MaxRecursionDepth`, default 250) applied to member/call/index chains as well as parentheses.

## Impact
An EdgionPlugins author (namespace-scoped write access) can submit a short script whose compilation
consumes most of a worker stack. With slightly deeper caller frames, a debug build, a future compiler
change that enlarges `compile_expr`'s frame, or a smaller worker stack, the Controller or every
Gateway aborts. The overflow is invisible to the existing safety story because the documented guard
(`max_nesting_depth`) is believed to cover it.

## Suggested direction
1. Count chain depth in the parser: increment the nesting counter for each postfix `.`/`[` and each
   binary-operator fold in `parse_expr_bp` (or track left-spine depth explicitly) so the existing
   64-level limit covers every recursion the compiler will perform. Return a normal `ParseError`.
2. Alternatively (or additionally) add a depth counter to `compile_expr` with a fixed limit
   (e.g. 128), and make `Expr` drop iterative or bounded by the same limit.
3. Add a regression test that compiles the maximal-node chain on a 256 KiB thread and expects an
   ordinary parse error.

## Notes
- Checked `skills/04-review/dsl/` (intentional-design and regression-guards) and
  `skills/01-architecture/02-gateway/dsl-engine-security-model.md:151`, which describes
  `max_nesting_depth` as the "parser stack-overflow guard". No page addresses chained expressions or
  compiler recursion; no task under `tasks/` mentions it.
- Not related to `gw-ext-08` (operator bytecode): this path is the ordinary `source` compile.

## Verification (independent)

Verdict: CONFIRMED. Severity P2 kept.
- `parse_expr_bp` (`edgion-dsl/src/parser/expr.rs:30-104`) loops over `.`/`[`/binary operators without `with_nesting`; only the index key is counted. `max_nesting_depth: 64`, `max_ast_nodes: 2_000` (`parser/mod.rs:43-45`).
- Re-ran the scratchpad probe (`scratchpad/dslprobe`, release build against current `edgion-dsl`): 1,996 `.len` segments abort with "stack overflow" on a 1.25 MiB thread and complete with "too many instructions" on 1.5 MiB / 2 MiB stacks; same for an `+x` chain.
- No `thread_stack_size` is configured in controller/gateway/common. Kept at P2: release builds on default 2 MiB workers currently survive with ~0.5 MiB headroom, so this is a latent abort, not a present crash.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- No file under `edgion-dsl/` changed since feature-08-30; the parser still guards nesting only in `with_nesting` and the compiler recursion is unchanged.
- `dsl-engine-security-model.md` changed only for fred -> redis-rs wording; no compiler-depth rationale was added.
