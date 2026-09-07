# B0117 — WebhookKeyGet Plugin

## Scope
- Baseline: `c1393c04`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Candidate owned file: `edgion-gateway/src/plugins/http/webhook_key_get/plugin.rs`
- Baseline blob: `9c121aa3d45057ac340ea8395073749440885005`
- Candidate implementation allow-list: not frozen until analysis completes
- Forbidden files: every file outside the eventual exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary
- Trace configuration preparation, PluginRuntime ownership, request execution, Webhook LinkSys calls, KeyGet/template evaluation, responses, failure policy, hot-reload generations, logs/access-log diagnostics, tests, and sibling implementations.
- Preserve canonical outbound HTTP/TLS/egress ownership, response-body streaming boundaries, redaction, bounded memory, binary-owned logging, canonical access-log observability, and cross-cutting unification.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be added without editing an existing file.

## Handoff
- Analysis: change, simple. The production-only resolver trait has one trivial implementation and exists solely for inline test injection; `async_trait` plus dynamic dispatch adds one boxed future and vtable call per configured variable on every request. The fallback path also clones the configured default before the production session copies it into the context map.
- Candidate correction: delete the resolver trait, production wrapper, boxed generation field, and injected constructor; call the canonical resolver directly. Extract a small synchronous outcome/application helper for unit tests, borrowing resolved/default values until the one required context write.
- Preserved behavior: serial configuration order, one live LinkSys lookup per variable, path extraction, default-or-skip, unconditional `GoodNext`, context overwrite, static value-free diagnostics, canonical outbound/TLS/egress/templates, and bounded separate-exchange response aggregation.
- Deferred candidates: the established always-degrade failure shape versus the general new non-auth dependency rule needs a separate product decision; the absent traffic-level dynamic hot-reload integration scenario must be independently validated and, if confirmed, tracked without editing existing integration files.
- Review: pending.
- Verification: pending.
- Commit: pending.
