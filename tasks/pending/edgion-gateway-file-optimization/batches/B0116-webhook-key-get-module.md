# B0116 — WebhookKeyGet Module Facade

## Scope
- Baseline: `c1393c04`
- Pipeline state: commit-ready
- Active anchor crate: `edgion-gateway`
- Candidate owned file: `edgion-gateway/src/plugins/http/webhook_key_get/mod.rs`
- Baseline blob: `2d0f95f90380199cdf1c4c0727913f21fa891e13`
- Candidate implementation allow-list: not frozen until analysis completes
- Forbidden files: every file outside the eventual exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary
- Read the facade in the context of the complete WebhookKeyGet module, PluginRuntime construction/execution, shared webhook/HTTP-client/template/key-extraction mechanisms, configuration validation, tests, and review precedent.
- Preserve canonical outbound HTTP, egress, failure-policy, response, logging, access-log, redaction, and observability ownership.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be added without editing an existing file.

## Handoff
- Analysis: examined-no-change. The file is the minimal conventional private-module/public-type facade, has no runtime behavior or hot-path cost, and is required by `plugins/http/mod.rs` and PluginRuntime. Inlining, changing visibility, or moving plugin behavior here would be structural churn without concrete value.
- Coverage: the complete plugin, resource and Controller validation, PluginRuntime construction/dispatch, LinkSys webhook manager and outbound stack, templates, KeyGet, hot-reload publication, logs/access-log diagnostics, unit tests, sibling facades, and frozen integration coverage were traced at `c1393c04`.
- Deferred candidate: shared outbound `PresencePolicy::WarnIfMissing` is currently silent and affects both WebhookKeyGet and ForwardAuth. It requires a separate shared-outbound batch with bounded value-free PluginLog/access-log diagnostics; do not implement it through this facade.
- Freshness: revalidated at `b09156e3` under the current root rules. B0115 changed only the Wasm VM manager, while intervening outbound HTTP/TLS/pinned-resolver changes add tests or AllEndpointStatus/backend-specific seams and leave Webhook's canonical client/TLS/egress path unchanged. The target blob remains `2d0f95f9`; the no-change decision and separate shared-outbound deferred classification remain valid.
- Review: independent no-change review approved with no Must Fix or Should Fix. The reviewer confirmed the live re-export, zero runtime cost, facade convention, full validation/runtime/hot-reload/outbound boundaries, and the separate low-severity shared `WarnIfMissing` defect. D0078 owns that nonduplicate cross-consumer work.
- Verification: four Gateway WebhookKeyGet tests, eleven shared outbound-request tests, and twelve resource WebhookKeyGet tests passed. `cargo fmt --all` and check mode, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and 3,084-file frozen integration guards passed on `b09156e3`; Clippy retained only existing warnings. Focused crate-local coverage is sufficient for D0078, so no integration issue is needed.
- Commit: `d5b52e17`.
