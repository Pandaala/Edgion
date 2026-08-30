---
name: ctx-set-design-decisions
description: Use when reviewing ctx_set plugin findings; covers cross-rule ctx variable reads, template injection scope, and other design decisions that are intentional and not accepted as bugs.
---

# ctx_set Plugin Review Decision Rules

> Source: 2026-04 plugin security audit; conclusions verified one-by-one against source code.

---

## Cross-Rule Reads of ${ctx:name} Are Design Intent, Not Vulnerability

**False-positive scenario**: ctx_set's `run_request` executes each rule in the vars list in order; after each rule completes, it immediately calls `session.set_ctx_var()`. Subsequent rules can use `${ctx:name}` in their templates to read values set by earlier rules, forming an implicit data flow.

**Reality**:

1. **This is the core value of `${ctx:name}` syntax**: If cross-rule reads were not allowed, `${ctx:name}` would have no reason to exist — other sources like from/header/query are already sufficient for a single rule's needs.
2. **Industry standard practice**: APISIX's `set-by-lua`/variable system, Kong's `ctx` shared memory, Envoy's dynamic metadata all allow plugins/rules to read variables set earlier; ordering dependency is known semantics.
3. **No injection risk**: SEC-01 has been fixed (template precompilation); user input is passed via the `CompiledTemplate` structure; `${}` is not parsed at runtime, so there is no second-pass expansion attack path. The `test_template_injection_prevention` test covers this scenario.

**Verdict**: Design intent, no fix needed. Documentation should make explicit: "rules execute in declaration order; subsequent rules can read variables set by earlier rules via `${ctx:name}`".

---

## Template Injection (SEC-01) Has Been Fixed via Precompilation

**Original issue**: `resolve_template` used a `while let Some(start) = result.find("${")` loop for replacement; if the replaced value contained `${...}` it would be expanded a second time. An attacker sending `X-User: ${header:Authorization}` could leak other request headers.

**Fix**: At config load time, templates are precompiled into `Vec<TemplateSegment>` (`Literal` / `Variable`); at runtime only segment iteration and concatenation is done, with no `find("${")` search at all. Equivalent to the SQL prepared statement approach — structure and data thoroughly separated.

**Files involved**:
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/ctx_set.rs` — `CompiledTemplate`, `TemplateSegment`
- `edgion-gateway/src/plugins/http/ctx_set/plugin.rs` — `evaluate_template`, `resolve_var`

**Verdict**: Fixed. In future reviews, if a ctx_set template-related issue is seen again, first confirm whether the precompiled path is in use.

---

## `apply_transform` Replace `None` Fallback Is Graceful-Degrade, Not Missing a `debug_assert!`

**False-positive scenario**: a finding claims `apply_transform`'s `Replace` arm needs a `debug_assert!(false, ...)` / `unreachable!()` tripwire because the `None` branch is documented as "should not happen after validate()" but has no compile-time / debug-time guard.

**Reality**:

1. **The real defect is already fixed.** The original P3-g4-006 report described a per-request `Regex::new` fallback. That regression was removed in commit `9074869` — the `None` arm no longer compiles a regex; it now emits `plugin_log.push_err("transform regex missing")` and returns `None`, letting the rule fall through to its declared `default`.
2. **The current shape is the project's canonical idiom for unreachable invariant branches on long-running paths.** See [`03-coding/02-rust-coding-rules.md` §Rule 5](../../03-coding/02-rust-coding-rules.md): data-plane filters graceful-degrade with `plugin_log.push_err` + safe default; they do not panic.
3. **The concrete plugin path has no useful debug-assert test surface.** A
   workspace search (`rg -n 'debug_assert!' edgion-*/src --glob '*.rs'`) finds
   assertions in infrastructure such as radix builders, path normalization,
   conf-sync, and CLI initialization, so a tree-wide rarity claim is not part of
   this verdict. `edgion-gateway/src/plugins/` has no current macro invocation,
   and every `apply_transform` test calls `validate()` first; a new assertion in
   the post-validation fallback would never fire in CI. The observable
   `plugin_log.push_err` plus safe default is the relevant project rule; see
   [`03-coding/02-rust-coding-rules.md` §Rule 5](../../03-coding/02-rust-coding-rules.md).
4. **`unreachable!()` is wrong on the data plane.** It panics in release too; one bad config would 5xx every in-flight request that hits this rule. `unreachable!()` belongs on exhaustive `match` arms of closed enums, not on validator-bypass fallbacks.

**Verdict**: Closed (P3-g4-006). The `None` arm is intentionally graceful-degrade. Refactoring suggestions to add `debug_assert!` / `unreachable!()` / `panic!()` are not accepted. Re-reports require evidence that (a) the per-request `Regex::new` regression has returned, or (b) the existing graceful arm has lost its `push_err` observability.
