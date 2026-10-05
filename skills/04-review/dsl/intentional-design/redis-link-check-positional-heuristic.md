---
name: redis-link-check-positional-heuristic
description: Use when a reviewer flags `check_redis_link_names` (edgion-dsl script.rs) bytecode `i-argc` link-name guess as misaligning / false-positive-rejecting valid configs / fail-closing the route — covers why the check is now fail-open and which fixes are not accepted.
knowledge_type: intentional-design
---

# `check_redis_link_names` Positional Heuristic — FIXED (fail-open), PIN-IN-PLACE

**Status**: The fail-open policy from plugins-http-dsl-F015 (2026-05-26) remains; the original single-push proof has since been narrowed for control-flow entry. See `check_redis_link_names`, `is_single_push_op`, and `has_jump_into_window` in `edgion-dsl/src/script.rs`.
**Tests**: `edgion-dsl/tests/unit/script_test.rs` covers multi-instruction arguments, valid serialized bytecode with forward-jump and loop-back ingress, a direct-check conditional-jump case, and a provable unknown literal.

## Conclusion

The compile-time redis link-name check is a **fail-open** bytecode heuristic. It guesses the link-name argument position as `i - argc` (the deepest push before a `redis.* CallBuiltin` at index `i`). It enforces `linkSysRefs` membership only if every opcode in `[i-argc, i)` is a zero-input single-value push **and** no `Jump`, `JumpIfFalse`, or `LoopBack` can target `(i-argc, i]`. A jump to the window's first opcode is safe because that push still executes. Any ambiguous layout, control-flow entry, or invalid jump target skips the check; the runtime `ok=false` path remains responsible for undeclared links. A positional miss must never reject a valid config.

The original mid-expression false positive was fixed by the single-push guard. That guard alone was insufficient for serialized bytecode with jumps. Verify both the window and control-flow guards before treating a new positional false positive as historical.

## Core Rationale

**1. Why the old heuristic fail-closed valid configs**

`check_redis_link_names` runs inside `DslConfig::compile_and_validate`, which `DslConfig::validation_error()` calls. A non-`None` validation error pushes the plugin into `invalid_plugins`; `seal_fail_closed` then prepends a `RejectFilter` that returns **500 on 100% of traffic** through the route. So a misaligned heuristic match converted a valid DSL plugin into a full-route outage at config-reload time.

The misalignment: the compiler compiles arguments with plain `compile_expr` in source order, so any argument can emit multiple instructions. For `redis.get("cache", req.header("X-Key"))`:

```
LoadConst("cache")          pos 0   <- arg[0] (link)
LoadConst("X-Key")          pos 1   <- arg[1] sub-expr part 1
CallBuiltin(ReqHeader, 1)   pos 2   <- arg[1] sub-expr part 2
CallBuiltin(RedisGet, 2)    pos 3 = i, argc = 2
```

`i - argc = 1` → `LoadConst("X-Key")`, which is not in `linkSysRefs`, so the old code returned `Err` even though `"cache"` is correctly declared.

**2. Why the single-push window also needs a control-flow guard**

The original conclusion that an all-single-push window *by itself* proves the first argument was too strong. A validated bytecode program may jump past the apparent first push and enter at a later push in the window. For example, `LoadConst("cache"), Jump(1), LoadConst("typo"), LoadConst("key"), CallBuiltin(RedisGet, 2)` reaches the call with `"cache"` and `"key"`, although the positional scan sees `"typo"` at `i-argc`. The validator checks jump structure and stack heights, but accepts this control flow. Rejecting `"typo"` would fail-close a valid plugin.

When every opcode in the window is a single push and no jump targets its interior or the call, sequential execution from the window's start supplies the `argc` topmost arguments in order. A target at the window start still executes every push. A target after it may skip a push, so the static check must abstain. The scan also abstains for invalid jump targets when called directly on unvalidated bytecode; the normal `resolve_script` path rejects those targets in `Validator` first. This remains a bounded best-effort check, not whole-program value provenance analysis.

**3. Why a bytecode-level fix, not the issue's "move to the AST/compiler layer"**

`check_redis_link_names` operates on a `CompiledScript` and is reached for **both** input paths: source-compile *and* direct pre-compiled `bytecode` (the production path — `resolve_script` prefers `bytecode`). An AST-only check in the compiler would not run for a config that ships pre-compiled `bytecode`, so a typo'd link in a bytecode payload would go unchecked. The bytecode-level guard covers both paths with no change to the serialized bytecode format or the compiler.

## Fix Suggestions Not Accepted

- **"Just drop the `return Err` path entirely (fail-open everything)"** (the issue's interim suggestion): unnecessarily guts the check for the common all-literal case it was designed for (typo detection). The window guard keeps the check active when it is provably correct.
- **"Move the check to the AST/compiler layer"**: does not cover the direct-`bytecode` input path (see Rationale 3). Allowed only as an *additional* layer, never as a replacement for the bytecode-level guard.
- **"Add the opcode to a denylist of stack-consuming ops"**: the implementation is an explicit *allowlist* on purpose — unknown / future opcodes must default to "not a single push" so a new opcode can only make the check *more* conservative, never re-introduce a false positive.
- **"Rely on the single-push window alone"**: serialized bytecode can branch into its interior while passing the validator's stack-height checks.

## Re-evaluation Triggers

Re-open only if:

- A new zero-input single-push `OpCode` variant is added and `is_single_push_op` is not updated (the check silently skips cases it could enforce — a correctness-of-coverage regression, never a false positive).
- A new control-flow opcode or jump-target convention is added; update the ingress check before relying on the positional proof.
- The redis link name is ever made a runtime-dynamic (non-literal) value — then static checking is impossible by construction and the runtime `ok=false` path is the only guard.

## Reference Cases

- Original finding: `tasks/plugins/plugins-http-dsl-F015-redis-link-check-false-positive-risk.md` (High / BadLogic).
- Related DSL builtin review note: [[chrono-display-panic]].
- Fail-closed wiring: `seal_fail_closed` / `RejectFilter` in `edgion-gateway/src/plugins/runtime/`.

## Resolution references

- `plugins-http-dsl-F015` — 2026-05-26.
