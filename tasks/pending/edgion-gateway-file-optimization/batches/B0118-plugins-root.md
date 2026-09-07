# B0118 — Plugins Root Facade

## Scope
- Baseline: `884069ac`
- Pipeline state: commit-ready
- Active anchor crate: `edgion-gateway`
- Candidate owned file: `edgion-gateway/src/plugins/mod.rs`
- Baseline blob: `369424a3465762a76f8eea12fb22b0c122714cd8`
- Candidate implementation allow-list: not frozen until analysis completes
- Forbidden files: every file outside the eventual exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary
- Read the root facade with HTTP/stream plugin module ownership, PluginRuntime construction and dispatch, public re-exports, binary callers, validation boundaries, tests, and review precedent.
- Preserve Controller/Gateway separation, canonical request/stream behavior, logging/access-log ownership, redaction, observability restraint, body/streaming invariants, and cross-cutting unification.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be added without editing an existing file.

## Handoff
- Analysis: examined-no-change. The 17-line root is a compile-time namespace and selective convenience facade with no request-path work, allocation, locking, validation, logging, or runtime cost. Existing module visibility and re-exports have live internal or extension-facing ownership; pruning, symmetry expansion, visibility churn, and wording-only edits do not clear the value gate.
- Coverage: HTTP/gRPC plugin compilation/publication and request/response dispatch, stream/TCP/UDP/TLS enforcement, route/runtime consumers, binary linkage, nested facades, validation, unit tests, frozen integration coverage, and review precedent were traced at `884069ac`.
- Deferred observations: three `Instant::elapsed()` calls belong to the later `plugin_runtime.rs` batch. The architecture page's obsolete `PluginRuntime` clone claim is knowledge drift and must be independently validated before tracking outside this facade batch.
- Freshness: revalidated from `640904be` through final observed HEAD `4019617b`; the target blob remains exactly `369424a3`. Intervening EdgionConfigData, WebhookKeyGet, IP-restriction, testing, and knowledge commits do not change the facade, exports, or decisive call chains.
- Value gate: examined-no-change remains correct. The complete 17-line facade only declares four modules and selectively re-exports live runtime/log/stream types; it performs no allocation, locking, validation, logging, dispatch, or request/connection work. Pruning, symmetry expansion, visibility churn, or wording-only edits would be unsupported API/style churn.
- Deferred disposition: D0079 separately owns the confirmed nonduplicate drift across the plugin-system architecture page, HTTP plugin-development guide, and plugin-runtime review precedent: removed empty-runtime `Clone`, removed forwarding constructors, and the distinct clone-cheap stream-runtime contract. The three `Instant::elapsed()` calls remain for the later `plugin_runtime.rs` batch.
- Integration disposition: no integration issue is needed for a compile-time facade with no behavioral change; existing crate compilation, 43 PluginRuntime tests, eight stream-runtime tests, and frozen integration coverage exercise its live imports and runtime consumers.
- Review: independent no-change review at `4019617b` approved the unchanged `369424a3` target blob. It found no code Must Fix or Should Fix after tracing all root exports and HTTP/gRPC/stream consumers. Its initial record-only Must Fix corrected D0079's evidence ownership and exact three-file documentation scope; re-review returned LGTM with no remaining finding.
- Verification: 43 PluginRuntime tests and eight stream-runtime tests passed in the independent review. Locked workspace all-target check and Clippy, locked Gateway binary build, format, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and frozen 3,084-file integration guards passed at `4019617b`; Clippy retained only existing warnings.
- Commit: pending.
