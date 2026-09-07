# D0079 — Correct PluginRuntime Architecture Documentation

## Meta

- Status: needs-implementation
- Severity: low
- Discovered in: B0118 plugins root facade analysis
- Category: architecture-knowledge-drift
- Canonical task: [`tasks/todo/correct-plugin-runtime-architecture-documentation.md`](../../../todo/correct-plugin-runtime-architecture-documentation.md)

## Evidence

Three source-of-truth pages have related drift:

- `skills/01-architecture/02-gateway/05-plugin-system.md` says `PluginRuntime::clone()` returns an empty runtime and that deprecated forwarding constructors remain. The current type intentionally has no `Clone`, request and route generations share it through `Arc<PluginRuntime>`, and only scoped route constructors remain.
- `skills/01-architecture/02-gateway/12-edgion-plugin-dev.md` also says deprecated forwarding constructors remain, although current source exposes only the scoped constructors.
- `skills/04-review/plugin-runtime/intentional-design/plugin-runtime-god-match.md` correctly preserves the removed empty-runtime `Clone` rationale for `PluginRuntime`, but incorrectly says `StreamPluginRuntime` and `TlsRoutePluginRuntime` follow the same no-clone pattern. Both stream runtime types currently derive `Clone` over `Arc`-backed chains.

## Why deferred

The B0118 target is a code-only namespace facade and none of these statements are owned by it. The correction should verify every current constructor and clone boundary, preserve the historical safety rationale, and update all three owning knowledge pages in one documentation-focused task.

## Integration-test impact

None. This is architecture documentation drift; source inspection, documentation validation, and relevant compile-time/unit evidence are sufficient. Existing integration tests remain frozen.
