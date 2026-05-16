---
name: plugin-runtime-god-match
description: Decide on "god object / huge file / should use registry / should use enum_dispatch / should split files by stage" findings about the central match dispatch in plugin_runtime.rs. Audited design tradeoff, will not fix.
---

# plugin_runtime.rs Central Match Dispatch — Audited Design Tradeoff

**Audit date**: 2026-04-26
**Linked issue**: P2-06 (closed, not fixed)
**Conclusion**: **The current central match dispatch is deliberately retained; "god object / should use registry / should use enum_dispatch / should split files by stage" fix suggestions are not accepted.**

## Code Involved

- `src/core/gateway/plugins/runtime/plugin_runtime.rs` (~987 lines)
  - `create_request_filter_from_edgion`
  - `create_upstream_response_filter_from_edgion`
  - `create_upstream_response_body_filter_from_edgion`
  - `create_upstream_response_from_edgion`
  - `get_plugin_validation_error`
  - `get_plugin_name`

## Points That Look Like Issues (none will be fixed)

### 1. One File ~900 Lines, 6 Parallel Central Matches

`EdgionPlugin` has 33 variants; each new HTTP plugin requires changes at 4 places in `plugin_runtime.rs` (`use` + construction match + validation match + name match). On the surface, a textbook god object.

**Why it will not be changed**:

- These matches are **exhaustive**; missing one variant causes the Rust compiler to error in multiple places. **Compile-time exhaustiveness is the core safety guarantee**; replacement with runtime registration (inventory/linkme) is forbidden.
- The central match provides three concrete values:
  1. **Plugin panorama in one place**: opening one file shows "which HTTP plugins the gateway supports and which stage each belongs to".
  2. **Stage constraint is explicitly centralized**: `CreateResult::WrongStage` explicitly expresses the deployment semantic of "which plugin is allowed in which stage". Trait-ification (default impl returning WrongStage) would make this constraint **invisible**.
  3. **Stage-specific error messages centralized**: messages like `"upstreamResponseFilterPlugins[{}] ({}): plugin is not valid for upstream response filter stage"` are clearer to maintain centrally than writing them in 33 trait impls separately.

### 2. "Should switch to `enum_dispatch` + per-plugin trait so adding a plugin requires no central change"

This suggestion **brings no performance benefit** and is not a clear DX win:

- **Hot path is untouched**: the current request path is already vtable calls on `Vec<Box<dyn RequestFilter>>`. `enum_dispatch` refactoring only touches the **central match at config-build time** (dozens of times per reload, nanoseconds), not the request path.
- **Touching the hot path would be a different refactor**: replacing `Vec<Box<dyn RequestFilter>>` with `Vec<RequestFilterPlugin>` enum. Even doing that, vtable savings are ~5-15ns/request — 3-6 orders of magnitude smaller than JWT verification (~10-100µs) / regex matching (~µs) / Redis rate limiting (~ms); **definitely not the gateway bottleneck**. Pingora itself uses trait objects extensively for the same reason.
- **DX net benefit is unclear**: mechanical migration of 33 variants has invisible regression risks (a trait impl with consistent signature but wrong content can compile); after trait-ification debugging is more dispersed (the stack shows `<XConfig as PluginConfigExt>::build_request` rather than the central function).

### 3. "Should split into 4 files by stage"

Does not solve the core pain (adding a plugin still requires changing the central dispatch); just splits the central match into 4 files. **Form-only refactor**, no substantive improvement.

## When to Re-Evaluate

Only consider rewriting when one of these triggers:

- ≥ 5 new HTTP plugins added in the next 3 months (refactor cost can be amortized).
- For other reasons (new stage model, new hook points), `plugin_runtime.rs` is being rewritten anyway; can be done along the way.
- New developers continually make mistakes at 4 central modification points.

Otherwise, **close new findings directly**, citing this document.

## Fix Suggestions Not Accepted (list)

- "Split into a plugin registry (inventory/linkme)"
- "Use `enum_dispatch` to disperse the dispatch table to each plugin file"
- "Split into 4 files by stage"
- "Introduce a `PluginConfigExt` trait to replace the central match"
- "God object, should be refactored"
- "Adding a plugin requires changing 4 central places" (this is a known trade-off, not a defect)

## Acceptable Small Improvements (if undertaken in the future)

- Use `strum::IntoStaticStr` derive to replace the hand-written match in `get_plugin_name` (**reduces 1** central match, keeps the rest).
- Move `get_plugin_validation_error` into the trait `PluginConfig::validation_error` (**reduces 1** central match, but each config must implement it).

These two changes do not break the centralization semantic; evaluate separately.

## Related: `PluginRuntime` deliberately does NOT implement `Clone` (g2-011 fix)

**Audit date**: 2026-05-15
**Linked issue**: g2-011 (closed, fixed)

If a finding suggests `PluginRuntime` should `#[derive(Clone)]` or "implement Clone properly," **close it**. The previous empty `impl Clone` (`fn clone(&self) -> Self { Self::new() }`) was a footgun: any misuse like `runtime_arc.as_ref().clone()` would silently strip the entire plugin chain from the request path with no compile-time or runtime signal. It was removed; sharing is done exclusively via `Arc<PluginRuntime>`, and rebuilds go through the `from_*` constructors at preparse.

A real `Clone` impl is also not feasible without forcing a `Clone` bound onto every `dyn RequestFilter` / `dyn UpstreamResponse*` trait object in the four stage `Vec`s — an unwanted constraint on every plugin implementation.

Sibling types `StreamPluginRuntime` and `TlsRoutePluginRuntime` already follow the same "no Clone" pattern; uniform treatment.
