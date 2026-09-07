# B0119 — Request Body Gate

## Scope

- Baseline: `86ea7498`
- Pipeline state: commit-ready
- Active anchor crate: `edgion-gateway`
- Candidate owned file: `edgion-gateway/src/plugins/runtime/body_gate.rs`
- Baseline blob: `b1f437f9722b5b16c5f614596c41dcb376361648`
- Candidate implementation allow-list: not frozen until analysis completes
- Forbidden files: every file outside an independently justified exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary

- Read the full gate, request-body storage/policy/context/session seams, every request plugin consumer, publication path, limits, spill cleanup, failure behavior, tests, and relevant review knowledge.
- Preserve the single request-scoped `BodyStorage`, resolved immutable request-body policy, physical/logical caps, response streaming, backpressure, redaction, binary-owned/canonical access-log observability, and cross-cutting unification.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be represented without editing an existing file.

## Handoff

- Analysis: change. Both request and defensive response gates call `BodyRequirement::max_body_size_bytes()` during execution; `parse_byte_size()` allocates a lowercase `String`, scans the unit, and parses `f64` for immutable configuration on every gated request or response. Construction also clones secondary conditions into `CompiledPluginConditions` while retaining the full raw requirement, duplicating the condition tree and limit string for the generation.
- Candidate correction: construct a compact private policy containing the parsed optional logical limit, failure mode, and directly moved/compiled nonempty secondary conditions. Hot paths compare the cached integer and no longer parse configuration. Keep the public HMAC/WAF/condition helper signatures and HMAC's security-sensitive ordering unchanged. Normalize an empty `PluginConditions` to `None` during construction.
- File structure: move the existing 25 inline tests out of the 934-line source into new sibling `edgion-gateway/src/plugins/runtime/tests/body_gate_test.rs` and add focused compiled-policy/empty-condition cases. The current source is about 69.9% inline tests, crossing the project externalization threshold.
- Preserved behavior: one request-scoped `BodyStorage`; resolved EGC physical cap; plugin logical cap only after capture with strict `body_len > limit`; capture-only versus materializing consumers; HMAC authenticate-before-capture order; fixed fail-close storage/physical/not-buffered cases; all status/failure/error mappings; streaming main response and defensive response-only rejection; no new observability or sensitive content.
- Complexity: simple/local. Exact implementation allow-list is `edgion-gateway/src/plugins/runtime/body_gate.rs` plus new `edgion-gateway/src/plugins/runtime/tests/body_gate_test.rs`.
- Freshness: promoted at `86ea7498` with unchanged target blob `b1f437f9` and no target/support diff.
- Deferred disposition: no duplicate or new deferred issue. HMAC's one post-auth helper parse remains outside scope absent profiling evidence; existing body/storage/hot-reload tasks remain preserved boundaries.
- Integration disposition: no existing integration test may change and no new integration issue is needed. Frozen BodyPlugins and DslBody suites already cover logical caps, spill/mutation, HMAC, replay, and secondary conditions; this is an equivalent construction-time optimization.
- Implementation: added private `BodyGatePolicy` construction-time compilation, removed retained raw requirements and duplicate condition trees from both gate structs, normalized empty conditions, and moved all 25 existing tests into the new sibling test file with two compiled-policy cases. Public helpers and every request/body/failure boundary remain unchanged.
- Review: independent final review at `86ea7498` approved source blob `660708be` and test blob `a8b08848` with no Must Fix, Should Fix, or deferred finding. It confirmed exact physical/logical cap ordering, lazy materialization, one BodyStorage, HMAC ordering, failure/error mapping, response streaming, generation ownership, observability, and test preservation.
- Verification: 27 focused body-gate tests passed. Locked workspace all-target check and Clippy, locked Gateway binary build, format, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and frozen 3,084-file integration guards passed at `86ea7498`; Clippy retained only existing warnings. Supplemental deny-warnings Clippy stopped only on unrelated existing `edgion-dsl` warnings.
- Commit: pending.
