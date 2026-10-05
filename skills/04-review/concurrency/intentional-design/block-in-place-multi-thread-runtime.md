---
name: dsl-block-in-place-multi-thread-runtime
description: Use when reviewing `DslPlugin::run_upstream_response_filter` `block_in_place`/`block_on` findings ("panics on current_thread runtime", "switch to spawn_blocking", "undocumented runtime constraint"); covers why the multi-thread requirement is by-design and the spawn_blocking fix is rejected.
knowledge_type: intentional-design
---

# DSL `run_upstream_response_filter` `block_in_place` Requires Multi-Thread Runtime — DESIGN-TRADEOFF (documented)

**Status**: Resolved by documentation — see the doc comment on
`DslPlugin::run_upstream_response_filter` in
`edgion-gateway/src/plugins/http/dsl/plugin.rs`.
**Test**: `edgion-gateway/tests/unit/plugins/http/dsl/mod_test/response_builtins.rs::test_upstream_response_filter_phase`
(uses `#[tokio::test(flavor = "multi_thread")]`).

## Conclusion

`DslPlugin::run_upstream_response_filter` bridges the **sync**
`UpstreamResponseFilter` trait to the async DSL VM via
`tokio::task::block_in_place(|| Handle::current().block_on(execute_safe(...)))`.
This REQUIRES a multi-thread Tokio runtime. The shipped gateway always builds
the Tokio default (multi-thread) runtime, so it is safe in production. The
constraint is now documented at the method level; no behavior change.

Fix suggestions of the form "replace `block_in_place` with `spawn_blocking`" or
"add a `Handle::current().runtime_flavor()` panic-guard at call time" are
**not accepted**.

## Core Rationale

**1. The sync trait is intentional; `block_in_place` is the only viable bridge**

`UpstreamResponseFilter` is deliberately sync (see the trait doc in
`edgion-gateway/src/plugins/runtime/traits/upstream_response_filter.rs`), even
though the current Pingora `upstream_response_filter` hook is async. The
historical dependency-based sync explanation was corrected during P090-009;
see [the current phase boundary](../../plugin-runtime/compatibility-constraints/filter-iteration-control-sync-phase-tradeoff.md). DSL is
the only impl that needs to run async work (redis / http builtins), so it must
bridge sync→async. On a multi-thread runtime a bare `Handle::block_on` from a
worker thread panics ("Cannot block the current thread from within a runtime");
`block_in_place` is what makes blocking legal on the worker. So `block_in_place`
is required, not incidental.

**2. Production is always multi-thread; the panic is test-only**

The gateway binary builds the Tokio default multi-thread runtime, so
`block_in_place` never panics in the shipped binary. The
`"Cannot use block_in_place inside a current_thread runtime"` panic can only be
hit by a future test or embedder that calls the method from a `current_thread`
runtime — i.e. at test-authoring time, not in production. The fix is therefore
to make the constraint visible (doc comment + the existing test comment), not to
restructure the data-plane code.

**3. `spawn_blocking` does not compile here**

`spawn_blocking` requires a `'static + Send` closure. `session: &mut dyn
PluginSession` and `log: &mut PluginLog` are non-`'static`, non-`Send` borrows
tied to the Pingora session; they cannot be moved into a `spawn_blocking`
closure without a channel-based handoff refactor that the original finding only
sketches (and which its own snippet flags with `// NOTE: session and log must be
Send; refactor if not`). The cost is large and the benefit (a slightly clearer
panic message in a test) is negligible.

## Fix Suggestions Not Accepted

- Replace `block_in_place` with `spawn_blocking` — does not compile; the borrows
  are non-`'static`/non-`Send`, and a channel handoff refactor is disproportionate
  to a test-only panic.
- Add a per-call `Handle::current().runtime_flavor()` check that panics with a
  clearer message — `run_upstream_response_filter` is on the per-response
  data-plane hot path; adding a check for a condition that is impossible in
  production violates hot-path minimalism, and Tokio's own panic message is
  already clear enough at test-authoring time.

## Re-evaluation Triggers

Re-open this decision only if:

- The gateway ever runs the DSL response stage on a `current_thread` runtime
  (e.g. a single-threaded embedding mode is shipped) — then a real bridge
  (channel handoff or making the trait async) becomes necessary.
- `UpstreamResponseFilter` is reworked into an async trait, removing the
  sync→async bridge entirely.

## Reference Cases

- Finding F-008 (`tasks/plugins/`), 2026-05-26 — documented, spawn_blocking rejected.
- Method doc: `edgion-gateway/src/plugins/http/dsl/plugin.rs`
  `DslPlugin::run_upstream_response_filter`.
- Sync trait contract: `edgion-gateway/src/plugins/runtime/traits/upstream_response_filter.rs`.
