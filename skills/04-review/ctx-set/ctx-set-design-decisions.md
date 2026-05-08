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
- `src/types/resources/edgion_plugins/plugin_configs/ctx_set.rs` — `CompiledTemplate`, `TemplateSegment`
- `src/core/gateway/plugins/http/ctx_set/plugin.rs` — `evaluate_template`, `resolve_var`

**Verdict**: Fixed. In future reviews, if a ctx_set template-related issue is seen again, first confirm whether the precompiled path is in use.
