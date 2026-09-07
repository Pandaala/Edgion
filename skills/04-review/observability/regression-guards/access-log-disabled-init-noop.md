---
name: access-log-disabled-init-noop
description: init_access_logger now installs a no-op AccessLogger (zero senders) when access logging is disabled or path is empty, so the startup path through get_access_logger_unchecked() can no longer panic and AccessLogger::send silently no-ops without touching the access_log_dropped metric. Reject re-reports of the disabled-mode startup panic and reject proposals to Option-propagate access_logger through ListenerContext / proxy structs.
knowledge_type: regression-guards
---

# observe F-B01 (access_log) — disabled mode installs a no-op logger instead of leaving OnceLock empty

**Status:** ✅ fixed · 2026-05-28
**Severity:** Medium (BadLogic) — Gateway process panicked at startup when `[access_log] enabled = false` (or `output.localFile.path` empty) because `init_access_logger` early-returned without setting the global `OnceLock`, and the very next startup step (`configure_listeners`) called `get_access_logger_unchecked().expect(..)`.

## Conclusion

`init_access_logger` (`edgion-gateway/src/observe/access_log/mod.rs`) now sets `ACCESS_LOGGER` unconditionally:

- enabled + valid path → `Arc<AccessLogger>` with the `local_file` sender, exactly as before;
- disabled (`enabled = false` or empty `path`) → `Arc::new(AccessLogger::new())` with zero senders, surfaced via the `"AccessLogger disabled, installing no-op logger"` info log.

`AccessLogger::send` (`logger.rs`) now early-returns when `self.senders.is_empty()` so the disabled-mode no-op silently drops without touching `edgion_access_log_dropped_total` — disabled is an operator choice, not a fault. The existing F-003 semantics for the **all-registered-senders-unhealthy** state are preserved: once at least one sender is registered, the loop still falls through to `global_metrics().access_log_dropped()` when none reports `healthy()`.

`get_access_logger()` keeps its `Option<&'static Arc<AccessLogger>>` shape (used by `request_mirror::mirror_log`). `get_access_logger_unchecked()` retains its `expect`; the doc comment now states the contract explicitly — safe after `init_access_logger` returns `Ok`, which the gateway startup sequence (`cli/mod.rs` step 20) always runs before any listener wiring or request processing begins.

## Core Rationale

**1. The "disabled = uninitialized OnceLock" coupling was the actual bug**

`init_access_logger` is the only producer of `ACCESS_LOGGER`, and the startup sequence (`cli/mod.rs` step 21 → `create_and_configure_server` → `GatewayBase::configure_listeners` → `self.get_access_logger()` → `get_access_logger_unchecked()`) is the only early consumer. There is no realistic deployment in which "logger truly uninitialized" is a separable signal from "logger disabled by config" — both ultimately mean "no access log lines will be written". Collapsing the two by installing a no-op `Arc<AccessLogger>` removes the panic surface and changes no operator-observable behavior (no file is opened, no metric counter advances, no listener wiring is altered).

**2. No-op logger > Option-propagation across the whole pipeline**

The alternative path the original finding floated — make `get_access_logger()` return `Option<Arc<AccessLogger>>`, change `ListenerContext.access_logger`, `EdgionHttp.access_logger`, `EdgionHttpRedirectProxy.access_logger`, `EdgionTlsProxy.access_logger`, `EdgionTlsPassthroughProxy.access_logger`, and `proxy_http::EdgionHttp.access_logger` all to `Option<Arc<...>>`, and guard every `.send(...)` site (`pg_logging.rs:64`, `redirect_http.rs:207`, `tls/proxy.rs:435`, `tls/passthrough_proxy.rs:435`, `pg_connected_to_upstream.rs:55`) with `if let Some(logger)` — touches 6+ files plus the `redirect_http` unit tests, with no operator-visible benefit. The no-op `AccessLogger::new()` is the same pattern `redirect_http.rs::test_access_logger` already uses for test wiring, so the precedent is consistent. Same redundancy-only refactor class as [protocol-logger-oncelock-pattern-not-extracted.md](../intentional-design/protocol-logger-oncelock-pattern-not-extracted.md): the alternative doesn't reduce surface, only spreads it.

**3. The `is_empty()` guard preserves F-003's metric contract**

`AccessLogger::send`'s `if self.senders.is_empty() { return; }` is the **only** new condition. The all-registered-senders-unhealthy path ([access-log-all-senders-unhealthy-metric.md](access-log-all-senders-unhealthy-metric.md)) still increments `edgion_access_log_dropped_total` because that fires only when the for-loop runs at least once and exhausts. Counting disabled-mode no-op sends as "drops" would spam the counter at request rate and conflate operator opt-out with writer failure — that's an inversion of the operator's monitoring intent, not a defense-in-depth.

**4. `_unchecked` stays — programmer-error vs operator-error**

The `expect("AccessLogger not initialized")` in `get_access_logger_unchecked()` is retained as a programmer-error tripwire: callers must run after `init_access_logger`. With the no-op-on-disabled fix, the only way to hit the panic is to call `_unchecked` before the startup sequence reaches step 20 — a bug in the call site, not an operator config. The original finding's "remove or deprecate `_unchecked`" suggestion is **not accepted**: deleting it would force the entire listener-wiring path onto the `Option`-propagation refactor (see rationale 2); deprecation without replacement adds churn without payoff.

## Fix Suggestions Not Accepted

- "Change `get_access_logger()` to return `Option<Arc<AccessLogger>>` and propagate `Option` through `ListenerContext` / all proxy structs / `.send` call sites." Not accepted — see Core Rationale 2. The no-op logger is the smaller-blast-radius equivalent fix and uses an existing pattern.
- "Remove or deprecate `get_access_logger_unchecked` once no callers remain." Not accepted — `base.rs::GatewayBase::get_access_logger` is a legitimate caller and the unchecked variant's contract ("safe after `init_access_logger`") is documented and enforced by the gateway startup sequence.
- "Add a panic-recovery / fallback layer to `configure_listeners` (catch_unwind around the access-logger fetch)." Not accepted — `init_access_logger` now provides the invariant; layering recovery on top of a fixed invariant is dead code.
- "Add `tracing::warn!` on the disabled-mode `send` path so operators see ongoing drops." Not accepted on two grounds: (a) operator-chosen disabled mode is not a fault to warn about; (b) `AccessLogger::send`'s call sites are on the data-plane hot path, where Iron Rule 3 ([observability-review-rule.md](../mandatory-design/observability-review-rule.md) · "Zero Tracing on the Data Plane") forbids any `tracing::*` regardless of rate-limit framing.
- "Count disabled-mode sends as `access_log_dropped` so the counter shows total volume." Not accepted — the counter exists to signal **unintended** log loss (queue-full, writer death, all-unhealthy); operator opt-out is the opposite. Adding a separate "intended-skip" counter only fragments the dashboard.

## Re-evaluation Triggers

Re-open only if:

- A second non-`local_file` sender is added whose `send()` failure semantics make the empty-senders carve-out ambiguous (e.g., a sender that's "registered but disabled" rather than "not registered").
- The startup sequence reorders such that some caller of `get_access_logger_unchecked()` runs **before** `init_access_logger` (then the contract is the bug, not the invariant).
- The "access log disabled" mode gains operator-visible semantics beyond "no file written" (e.g., a metrics-only sink) — that changes "no-op = correct" into a real feature requiring its own type.

## Reference Cases

- observe F-B01 (`tasks/observe/observe-FB01-get-access-logger-unchecked-panic-if-disabled.md`, 2026-05-28)
- [[access-log-all-senders-unhealthy-metric]] — sibling fix at the dispatcher level (F-003); preserved by the `is_empty()` guard
- [[protocol-logger-oncelock-pattern-not-extracted]] — same class of "minimal change beats wide refactor" decision for the four async logger OnceLock sites
- `edgion-gateway/src/cli/mod.rs:285,294` — startup ordering that makes `init_access_logger` the precondition for `get_access_logger_unchecked()`
- `edgion-gateway/src/runtime/server/base.rs:38-40` — the single caller of `get_access_logger_unchecked()` on the listener-wiring path

## Resolution references

- `observe-FB01` — 2026-05-28.
