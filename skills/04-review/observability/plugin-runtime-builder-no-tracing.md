---
name: plugin-runtime-builder-no-tracing
description: Use when reviewing findings that `PluginRuntime::add_upstream_response_body_filter` (or any other `add_*` builder on `PluginRuntime`) should emit a `tracing::info!`/`debug!` for the added filter; explains why the per-builder tracing was removed and why the four sibling builders must stay tracing-free regardless of current call-site context.
---

# `PluginRuntime` `add_*` Builders Are Tracing-Free (Fixed)

## Conclusion

The four `add_*` builder helpers on `PluginRuntime`
(`src/core/gateway/plugins/runtime/plugin_runtime.rs`) —
`add_request_filter`, `add_upstream_response_filter`,
`add_upstream_response_body_filter`, `add_upstream_response` —
**must not** emit any `tracing::*` call. The single outlier
`tracing::info!("PluginRuntime: adding body filter '{}'", filter.name())` on
`add_upstream_response_body_filter` was removed on 2026-05-27, and the same
helper was demoted from `pub` to crate-private so all four builders share one
shape: `fn add_*(&mut self, …) { self.<vec>.push(…); }`.

Two grounds — both must hold for the rule:

1. **Builder shape, not config-load shape.** A pushing-into-`Vec` helper is a
   primitive; the appropriate place to log "this plugin was added" (if at all
   needed) is the explicit config-load function that decided to add it
   (`add_from_*_filters`, the preparse path at lines 286/294, etc.), where the
   "we are loading config" context is unambiguous and any future hot-path caller
   of the same builder cannot inherit the log.
2. **Iron Rule 3 proximity.** Today all callers of these builders are
   ConfHandler / preparse-time, so a `tracing::info!` inside the builder is not
   a live Iron Rule 3 violation. Making any of them `pub` lifts that guarantee:
   a future test helper, a dynamic-filter mutation path, or a misplaced
   per-request rebuild would silently emit `info!` (or worse) per body chunk,
   with no syntactic signal at the new call site. Keeping the builders
   crate-private **and** tracing-free is the protective pair; dropping either
   half re-opens the trap.

Plugin-count visibility for ops/debug is already covered by `total_plugin_count`
/ `*_plugins_count` accessors and by the per-stage validation errors collected
in `add_from_*_filters` — removing the builder-level `info!` does not blind any
existing observability surface.

## Decision Rule

When a review re-reports any of:

1. "`add_upstream_response_body_filter` should log the filter name on add for
   parity with controller-side plugin-load logs"
2. "the call is a *cold path* — only the preparse path calls it today, so
   Iron Rule 3 does not apply; restore the `tracing::info!`"
3. "`add_upstream_response_body_filter` should stay `pub` because a future test
   helper / dynamic mutator might want to call it"
4. "downgrade the `info!` to `debug!` / `trace!` instead of removing it"

→ close as **fixed + design intent**. Verify before closing:

- All four `add_*` helpers in `plugin_runtime.rs` are crate-private (`fn`, not
  `pub fn`) and contain only the `Vec::push` line — no `tracing::`, no other
  side effect.
- If any of them needs to be re-promoted to `pub` for a legitimate caller,
  audit the caller's invocation context first and surface the audit in the
  promoting PR; do **not** re-add tracing as part of that promotion.

Re-report **(2)** is **not accepted**: the rule is preventive precisely because
"current callers are cold" is a property of today's tree, not a stable
invariant. Re-report **(3)** is **not accepted** in isolation — if such a caller
arrives, it earns its own scope-gated change, not a speculative `pub` left in
place. Re-report **(4)** is **not accepted** for the same reason the DSL
`Vm::execute` tracing was declined ([dsl-vm-execute-tracing-declined.md](dsl-vm-execute-tracing-declined.md)):
the prohibition is on the path class (builder primitive sitting next to
per-request rebuild sites), not the level.

## Background Notes

- Decision date: 2026-05-27
- Source issue: `tasks/plugins/plugins-runtime-FB02-add-body-filter-tracing-info-proximity.md` (closed)
- Same class as [real-ip-run-request-tracing-removed.md](real-ip-run-request-tracing-removed.md)
  and [global-plugins-resolve-no-tracing.md](global-plugins-resolve-no-tracing.md):
  a data-plane-adjacent `tracing::*` callsite removed because per-request reach
  outranks the configured signal. This case is one step earlier in the chain —
  the builder itself, not the dispatch loop — and is fixed preventively rather
  than reactively.
