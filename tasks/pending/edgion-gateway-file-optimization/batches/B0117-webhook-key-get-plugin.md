# B0117 — WebhookKeyGet Plugin

## Scope
- Baseline: `c1393c04`
- Pipeline state: commit-ready
- Active anchor crate: `edgion-gateway`
- Candidate owned file: `edgion-gateway/src/plugins/http/webhook_key_get/plugin.rs`
- Baseline blob: `9c121aa3d45057ac340ea8395073749440885005`
- Implementation allow-list: only `edgion-gateway/src/plugins/http/webhook_key_get/plugin.rs`, including inline tests
- Forbidden files: every file outside the eventual exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary
- Trace configuration preparation, PluginRuntime ownership, request execution, Webhook LinkSys calls, KeyGet/template evaluation, responses, failure policy, hot-reload generations, logs/access-log diagnostics, tests, and sibling implementations.
- Preserve canonical outbound HTTP/TLS/egress ownership, response-body streaming boundaries, redaction, bounded memory, binary-owned logging, canonical access-log observability, and cross-cutting unification.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be added without editing an existing file.

## Handoff
- Analysis: change, simple. The production-only resolver trait has one trivial implementation and exists solely for inline test injection; `async_trait` plus dynamic dispatch adds one boxed future and vtable call per configured variable on every request. The fallback path also clones the configured default before the production session copies it into the context map.
- Candidate correction: delete the resolver trait, production wrapper, boxed generation field, and injected constructor; call the canonical resolver directly. Extract a small synchronous outcome/application helper for unit tests, borrowing resolved/default values until the one required context write.
- Preserved behavior: serial configuration order, one live LinkSys lookup per variable, path extraction, default-or-skip, unconditional `GoodNext`, context overwrite, static value-free diagnostics, canonical outbound/TLS/egress/templates, and bounded separate-exchange response aggregation.
- Deferred candidates considered: the established always-degrade failure shape versus the general new non-auth dependency rule, and the absent traffic-level dynamic hot-reload integration scenario.
- Freshness: revalidated at `d5b52e17` under current rules and the unchanged target blob `9c121aa3`. B0115 is Wasm-only; B0116 is record-only; intervening outbound HTTP/TLS/pinned-resolver work leaves Webhook's canonical path unchanged. The simple one-file change and all preserved invariants remain valid.
- Deferred disposition: D0078 already owns shared `WarnIfMissing` diagnostics. The established optional-enrichment failure shape is the concrete product contract rather than a new B0117 issue, and this allocation-only edit does not warrant a dynamic integration issue; existing frozen integration plus manager/store tests are proportionate.
- Implementation: removed the test-only boxed async resolver and its per-variable allocation/vtable dispatch, called the canonical resolver directly, selected resolved/default/skip outcomes through a borrowed pure helper, and removed the redundant configured-default clone before the sole context write. Four synchronous inline tests retain resolved/default/skip and ordered-overwrite coverage.
- Review: independent final review at `6ab45e17` approved the current worktree blob `eab426de` with no Must Fix or Should Fix. It confirmed exact resolution precedence, serial one-lookup-per-variable behavior, generation ownership, context/log/error ordering, unconditional `GoodNext`, canonical outbound/TLS/egress/template/capped-response ownership, redaction, and body-streaming boundaries.
- Verification: four focused Gateway WebhookKeyGet tests, 53 adjacent Gateway webhook tests, and twelve resource WebhookKeyGet tests passed. Locked workspace all-target check and Clippy, locked Gateway binary build, format, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and refreshed 3,084-file frozen integration guards passed at `6ab45e17`; Clippy retained only existing warnings.
- Integration disposition: no existing integration test changed. No new integration issue is warranted for this allocation-only refactor because the frozen suite already covers real resolution, fallback/skip, path override, templates, TLS, egress blocking, context propagation, and WebhookKeyGet-before-KeyAuth ordering. D0078 remains separately owned by the shared outbound builder.
- Commit: `374cdca1` (the shared branch re-recorded the exact `640904be` content while concurrent work synchronized HEAD).
