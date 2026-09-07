# B0130 — Plugin Runtime Façade

## Scope

- Baseline: `2cae2cfc59616782d5833ff8920e7c2158ba6bb1`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Candidate owned file:
  - `edgion-gateway/src/plugins/runtime/mod.rs`
- Baseline blob: `ae90bbd732fbd7fca744a483d2d04be0526e424d`
- Candidate implementation allow-list: not frozen until analysis completes
- Forbidden files: every file outside an independently justified exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary

- Read the complete runtime façade, every public and crate-private export, direct consumers, module ownership, tests, and relevant review knowledge.
- Preserve public API, plugin/runtime lifecycle, immutable publication, canonical access-log ownership, redaction, observability restraint, and streaming.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be represented without editing an existing file.

## Handoff

- Analysis: examined-no-change. The 32-line façade only declares 12 child modules and re-exports 32 live names; it performs no request work, allocation, synchronization, validation, logging, or initialization. All public and crate-private exports were traced, including the public `log` path required by the exported `la!` macro.
- Value decision: visibility pruning, alias removal, symmetry expansion, import reordering, façade-only tests, and comment polishing offer only public/API churn with no correctness, security, runtime, or maintenance benefit. Reorganizing the accepted central dispatch belongs to neither this file nor a façade cleanup.
- Existing ownership: D0079 already owns stale plugin-runtime architecture documentation, and the later full `plugin_runtime.rs` batch owns its timing sites. No new deferred or integration issue is needed.
- Review: pending.
- Verification: pending.
- Commit: pending.
