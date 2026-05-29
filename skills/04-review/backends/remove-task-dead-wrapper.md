---
name: remove-task-dead-wrapper
description: Read when reviewing remove_task being a dead one-line wrapper that unconditionally delegates to cancel_task / the two private health-check task helpers are redundant / inline remove_task into its single caller (backends-F011).
---

# backends-F011 — remove_task dead one-line wrapper

**Status / verdict**: fixed (backends-F011, 2026-05-24)

When reviewing findings of the form "`remove_task` is a dead one-line wrapper that unconditionally delegates to `cancel_task` / the two private health-check task helpers are redundant / inline `remove_task` into its single caller (backends-F011)": fixed (backends-F011, 2026-05-24).

`remove_task` (`src/core/gateway/backends/health/check/manager.rs`) is deleted; its single caller `reconcile_service` now calls `self.cancel_task(service_key)` directly. `cancel_task` (still private) remains the one place that removes the per-service `JoinHandle` and `abort()`s it, and is also called from the `upsert_task` no-runtime fallback branch. Pure no-behavior-change dead-wrapper removal (the wrapper added no logic, doc, or semantic distinction). The issue's "if a semantic distinction is ever needed, reintroduce with a doc comment" note stands but no distinction exists today. Same redundancy class as backends-F007 / backends-F008 / backends-F009 / backends-F010. Re-reports of the dead-wrapper form are not accepted; the inverse "reconcile_service calls cancel_task directly, add a dedicated remove method" is also not accepted unless it carries real cleanup logic (e.g. `health_store.unregister_service`, which already lives in the caller). Verify the current `manager.rs` has no `remove_task` before re-reporting.
