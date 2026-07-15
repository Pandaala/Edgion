---
name: dsl-coding
description: DSL plugin implementation skills for Edgion — builtin development, path/uri variable reference, redis/http builtin guides. Use when adding or modifying a `req.*`, `http.*`, or `redis.*` builtin, or wiring DslRuntime fields for a new namespace.
---

# DSL Plugin Implementation

> Implementation-level guides for the EdgionDSL plugin: how to use the existing builtins from script-author and gateway-developer viewpoints, and how to add a new function inside an existing namespace.
> Engine-internal architecture (VM, bytecode, panic isolation, DslRuntime lifecycle) lives in [`../../01-architecture/02-gateway/dsl-engine.md`](../../01-architecture/02-gateway/dsl-engine.md); user-facing syntax and the full function catalogue live under `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/`.

Ownership is deliberately split across three layers:

- `edgion-dsl`: parser, bytecode compiler, values/errors, and static validation.
- `edgion-resources`: `DslConfig` plus the shared compile/validate entry used during resource ingest.
- `edgion-gateway`: VM execution, host session integration, async builtins, and runtime provider dispatch.

Keep new code in the lowest layer that has the required dependencies; never move Gateway
runtime dependencies into the leaf DSL crate.

## File list

| File | Topic |
|------|-------|
| [builtins-path.md](builtins-path.md) | `req.path` / `req.uri` / `req.query` and related path/uri/query variables: views, behavior, rewrite interaction, allocation cost |
| [builtin-error-semantics.md](builtin-error-semantics.md) | Error-handling contract for DSL builtins — which functions return `nil` / `""` / `false` silently vs raise `RuntimeError::ApiError`; lock-in tests and "adding a new builtin" decision flow |
| [http-call.md](http-call.md) | `http.call()` builtin: async outbound HTTP, `Value::Map` return shape, SSRF guard, config fields, test patterns |
| [redis-builtin.md](redis-builtin.md) | `redis.*` builtins: async LinkSys Redis interaction, named alias lookup, `Map { ok, value, error }` return shape, "Adding new redis.* functions" 5-step recipe |

## Cross-references

- **Engine architecture** → [`../../01-architecture/02-gateway/dsl-engine.md`](../../01-architecture/02-gateway/dsl-engine.md): data flow, source layout, async/sync bridging, panic isolation
- **Sandbox security model (maintainer view)** → [`../../01-architecture/02-gateway/dsl-engine-security-model.md`](../../01-architecture/02-gateway/dsl-engine-security-model.md): where each `VmLimits` cap is enforced, ReDoS posture, SSRF guard rationale — read before adding a new builtin that produces strings/lists/maps or calls out to external services
- **Plugin system overview** → [`../../01-architecture/02-gateway/05-plugin-system.md`](../../01-architecture/02-gateway/05-plugin-system.md): where DslPlugin sits among other gateway plugins
- **URI normalization** (paired reading for `builtins-path.md`) → [`../../01-architecture/02-gateway/uri-normalization/SKILL.md`](../../01-architecture/02-gateway/uri-normalization/SKILL.md)
- **Value-type design memo** → [`../../04-review/dsl-value-str/sso-migration-not-worth-it.md`](../../04-review/dsl-value-str/sso-migration-not-worth-it.md): why `Value::Str(Arc<str>)` stays, why `Value::Map` is `Box`ed
- **User-facing function reference** → `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference.md`

## When to use which file

- Script-author question about which path variable to read → `builtins-path.md`
- Adding or debugging an outbound HTTP call from a script → `http-call.md`
- Adding or debugging a `redis.*` function (or wiring a new `redis.*` op end-to-end) → `redis-builtin.md`
- Anything touching the VM, bytecode, or compiler (not just a builtin) → fall back to `dsl-engine.md` first
