---
name: tls-route-fallback-no-tracing
description: Use when reviewing findings that `run_tls_route_plugins` should emit `tracing::debug!/info!` when it falls back from Stage 2 (`tls_route_plugin_runtime`) to Stage 1 (`stream_plugin_runtime`) compat path, so operators can distinguish "no ESP configured" from "Stage 1 compat active" in tracing output. Explains why the fallback branch is intentionally tracing-silent and observed only via the access-log `(compat)` suffix.
---

# `run_tls_route_plugins` Stage 2 → Stage 1 Compat Fallback Is Not Logged via Tracing

## Conclusion

`run_tls_route_plugins` (`src/core/gateway/routes/tls/mod.rs`) **must not** emit
`tracing::debug!` / `tracing::info!` (or any `tracing::*`) on the backward-compat
branch that falls back to `stream_plugin_runtime` when `tls_route_plugin_runtime`
is empty. Fix suggestions of the form

- "add a `tracing::debug!(store_key = %store_key, \"TlsRoute: no Stage 2 runtime found, falling back to Stage 1 (compat)\")` at the fallback site so operators can distinguish 'no ESP configured' from 'compat active' in log aggregation"
- "promote it to `tracing::info!` so the fallback is visible in default-level deployments"
- "wrap the branch with a one-shot `OnceCell` so the warn only fires once per `store_key` to avoid log-volume scaling"

are **not accepted**. The compat branch's activation is already observable via
the access-log entry written by `ctx.push_log("stream plugin allowed (compat)")`
(allow path) and `ctx.err_log = Some(format!("stream plugin denied (compat): {reason}"))`
(deny path) — both land in `tls.log` with the `(compat)` suffix. Tracing is the
wrong channel.

## Core Rationale

**1. `run_tls_route_plugins` is on the TLS connection hot path.**
The function is called once per accepted TLS connection from both
`src/core/gateway/routes/tls/proxy.rs` (terminate mode) and
`src/core/gateway/routes/tls/passthrough_proxy.rs` (passthrough mode). Both
files are explicitly listed under the "TCP/TLS connection path" entry of
`skills/03-coding/01-log-safety.md` Iron Rule 3, and Stage-2 plugin dispatch
under `plugins/stream/tls_route/*` is itself called out in the same rule's
"Plugin dispatch" extension. The rule forbids any `tracing::*` macro on this
path, **including `debug!`** — the `EnvFilter` filters out `debug!` at runtime
but the format-argument evaluation cost remains, and a single misconfigured
ESP (only `plugins`, no `tlsRoutePlugins`) would make the line fire on every
TLS connection that hits a `TLSRoute` referencing it — log-volume scaling with
the TLS connection rate, the same failure mode that motivated removing the
TCP-side warns in `tcp-stream-plugin-no-tracing`.

**2. The data-plane channel already distinguishes the two states.**
The `(compat)` suffix on the access-log line is the canonical distinguisher:

- Resource has `tlsRoutePlugins` and the Stage 2 runtime ran → access-log
  records `"tls route plugin allowed"` / `ctx.err_log = "tls route plugin denied: ..."`.
- Resource has only legacy `plugins` and the Stage 1 compat ran → access-log
  records `"stream plugin allowed (compat)"` / `ctx.err_log = "stream plugin
  denied (compat): ..."`.
- Resource has neither runtime populated → function returns `true` with no
  push to `ctx.push_log` / `ctx.err_log` from this branch (intentional
  no-policy pass-through, same as `stream_plugin_store_key = None`).

An operator who needs to confirm "is Stage 1 compat active for store_key X?"
greps `tls.log` for `(compat)` — bounded, per-connection, no extra hot-path
allocation, no EnvFilter dependency. This is the project's standing design
for data-plane observability (Iron Rule 3 alternatives table:
"Per-connection error/event → `ctx.err_log` / `ctx.add_error()`").

**3. Symmetry with the TCP / TLS rejection precedent.**
The 2026-05-24 fix recorded in `tcp-stream-plugin-no-tracing.md` removed
`tracing::warn!` from the TCP `check_stream_plugins` fail-close branches and
noted explicitly that "TLS counterpart `run_tls_route_plugins`
(`routes/tls/mod.rs`) already used the correct data-plane channel
(`ctx.err_log`); the TCP path diverged by writing the same diagnostic to
tracing". The F-011 proposal would reintroduce that divergence in the
opposite direction (add tracing on the TLS side that the TCP side just had
removed). Same class of rejection as
`global-plugins-resolve-no-tracing`,
`bandwidth-limit-invalid-rate-no-tracing`,
`plugin-runtime-builder-no-tracing` —
hot-path data-plane helpers do not get tracing, regardless of whether the
branch is "rare" (operator-misconfig only).

## Decision Rule

When a review re-reports any of:

1. "`run_tls_route_plugins` compat fallback (`if !stream_runtime.is_empty()`
   branch in `routes/tls/mod.rs`) is silent in tracing — add
   `tracing::debug!`/`info!` so operators can tell from logs which stage ran"
2. "the access-log `(compat)` suffix is invisible to operators who tail
   tracing output, add a tracing emission for parity"
3. "downgrade to `tracing::trace!` / gate with `cfg(debug_assertions)` / gate
   with a per-`store_key` `OnceCell` so it only fires once per fallback target"

→ close as **accepted tradeoff + design intent**. Verify before closing:

- `run_tls_route_plugins` in `src/core/gateway/routes/tls/mod.rs` contains no
  `tracing::*` calls in either the Stage 2 branch or the Stage 1 compat
  fallback.
- The compat fallback still pushes `"stream plugin allowed (compat)"` /
  `"stream plugin denied (compat): {reason}"` into `ctx.push_log` /
  `ctx.err_log` so the access-log distinguishing suffix is preserved.

Re-report **(3)** is **not accepted**: a `OnceCell` gate would technically
bound the log volume, but it would also mean the line never fires again after
the first occurrence (so operators investigating a later incident on the same
`store_key` would see nothing), and it still leaves the `format!`-arg
evaluation cost on the hot path. The data-plane channel already provides the
distinguisher with neither downside.

## Re-evaluation Triggers

Re-evaluate only if one of the following becomes true:

- Pingora's data-plane logging story changes such that hot-path `tracing::*`
  no longer carries per-request format cost (would also unlock the broader
  Iron Rule 3 review).
- The `(compat)` suffix is removed from `ctx.push_log` / `ctx.err_log` (would
  remove the canonical distinguisher and force a new channel).
- A metrics counter `edgion_tls_route_stream_compat_fallback_total{store_key}`
  is requested as a separate feature — that would be hot-path-safe and
  bounded-cardinality (`store_key` is operator-controlled), and would belong
  in `tasks/todo/` rather than as a fix to this finding.

## Background Notes

- Decision date: 2026-05-27 (plugins-stream-F011).
- Source issue: `tasks/plugins/plugins-stream-F011-tls-route-fallback-to-stage1-undocumented.md` (closed).
- The doc-gap part of the same issue was addressed separately: a "Compatibility Fallback" paragraph was added to the "Stage 2: TlsRoute" section of `docs/en/dev-guide/stream-plugin-development.md` and the zh-CN counterpart, covering when the fallback activates and the silent-stop-on-upgrade operational hazard.
- Same class as
  [`tcp-stream-plugin-no-tracing.md`](tcp-stream-plugin-no-tracing.md),
  [`global-plugins-resolve-no-tracing.md`](global-plugins-resolve-no-tracing.md),
  [`bandwidth-limit-invalid-rate-no-tracing.md`](bandwidth-limit-invalid-rate-no-tracing.md),
  [`plugin-runtime-builder-no-tracing.md`](plugin-runtime-builder-no-tracing.md):
  per-connection data-plane helpers do not get tracing, even when the branch
  is rare.
