---
name: redis-link-check-positional-heuristic
description: Use when a reviewer flags `check_redis_link_names` (DSL plugin.rs) bytecode `i-argc` link-name guess as misaligning / false-positive-rejecting valid configs / fail-closing the route — covers why the check is now fail-open and which fixes are not accepted.
---

# `check_redis_link_names` Positional Heuristic — FIXED (fail-open), PIN-IN-PLACE

**Status**: Resolved (plugins-http-dsl-F015, 2026-05-26) — see `check_redis_link_names` + `is_single_push_op` in `edgion-gateway/src/plugins/http/dsl/plugin.rs`.
**Tests**: `plugin.rs::tests::validate_config_accepts_redis_with_multi_instruction_sibling_arg` (regression for the false-positive 500) and `validate_config_skips_check_when_layout_is_ambiguous` (documents fail-open); `validate_config_rejects_unknown_redis_link` still locks the all-literal reject path.

## Conclusion

The compile-time redis link-name check is a **fail-open** bytecode heuristic. It guesses the link-name argument position as `i - argc` (the deepest push before a `redis.* CallBuiltin` at index `i`), but only enforces the `linkSysRefs` membership check when the layout is **provably unambiguous** — i.e. every one of the `argc` opcodes in `[i-argc, i)` is a zero-input single-value push (`is_single_push_op`: `LoadConst`, `LoadNil`, `LoadTrue`, `LoadFalse`, `GetLocal`). Otherwise it skips the check and lets the runtime `ok=false` path handle an undeclared link. A positional miss must **never** reject a valid config.

Findings of the form "`i - argc` can land mid-expression and wrongly reject a valid plugin / fail-close the route with 500" describe the **pre-fix** behavior and are not accepted against current code — verify the `is_single_push_op` guard is present before re-reporting.

## Core Rationale

**1. Why the old heuristic fail-closed valid configs**

`check_redis_link_names` runs inside `DslPlugin::validate_config`, which `DslConfig::validation_error()` calls. A non-`None` validation error pushes the plugin into `invalid_plugins`; `seal_fail_closed` then prepends a `RejectFilter` that returns **500 on 100% of traffic** through the route. So a misaligned heuristic match converted a valid DSL plugin into a full-route outage at config-reload time.

The misalignment: the compiler compiles arguments with plain `compile_expr` in source order, so any argument can emit multiple instructions. For `redis.get("cache", req.header("X-Key"))`:

```
LoadConst("cache")          pos 0   <- arg[0] (link)
LoadConst("X-Key")          pos 1   <- arg[1] sub-expr part 1
CallBuiltin(ReqHeader, 1)   pos 2   <- arg[1] sub-expr part 2
CallBuiltin(RedisGet, 2)    pos 3 = i, argc = 2
```

`i - argc = 1` → `LoadConst("X-Key")`, which is not in `linkSysRefs`, so the old code returned `Err` even though `"cache"` is correctly declared.

**2. Why the "all-single-push window" guard is sound, not another heuristic**

The guard is provably equivalent to "every argument is a single instruction": just before the call the stack holds exactly `argc` argument values. If the `argc` preceding opcodes are *all* zero-input single pushes, they add exactly `argc` values, so each argument maps 1:1 to one opcode and `i-argc` is provably `arg[0]`. Conversely, if any argument spans multiple instructions, its final (stack-consuming) opcode lands inside the `[i-argc, i)` window, so the window is *not* all single pushes and the check skips. No false positive survives.

**3. Why a bytecode-level fix, not the issue's "move to the AST/compiler layer"**

`check_redis_link_names` operates on a `CompiledScript` and is reached for **both** input paths: source-compile *and* direct pre-compiled `bytecode` (the production path — `resolve_script` prefers `bytecode`). An AST-only check in the compiler would not run for a config that ships pre-compiled `bytecode`, so a typo'd link in a bytecode payload would go unchecked. The bytecode-level guard covers both paths with no change to the serialized bytecode format or the compiler.

## Fix Suggestions Not Accepted

- **"Just drop the `return Err` path entirely (fail-open everything)"** (the issue's interim suggestion): unnecessarily guts the check for the common all-literal case it was designed for (typo detection). The window guard keeps the check active when it is provably correct.
- **"Move the check to the AST/compiler layer"**: does not cover the direct-`bytecode` input path (see Rationale 3). Allowed only as an *additional* layer, never as a replacement for the bytecode-level guard.
- **"Add the opcode to a denylist of stack-consuming ops"**: the implementation is an explicit *allowlist* on purpose — unknown / future opcodes must default to "not a single push" so a new opcode can only make the check *more* conservative, never re-introduce a false positive.

## Re-evaluation Triggers

Re-open only if:

- A new zero-input single-push `OpCode` variant is added and `is_single_push_op` is not updated (the check silently skips cases it could enforce — a correctness-of-coverage regression, never a false positive).
- The redis link name is ever made a runtime-dynamic (non-literal) value — then static checking is impossible by construction and the runtime `ok=false` path is the only guard.

## Reference Cases

- Original finding: `tasks/plugins/plugins-http-dsl-F015-redis-link-check-false-positive-risk.md` (High / BadLogic).
- Related DSL builtin review note: [[chrono-display-panic]].
- Fail-closed wiring: `seal_fail_closed` / `RejectFilter` in `edgion-gateway/src/plugins/runtime/`.
