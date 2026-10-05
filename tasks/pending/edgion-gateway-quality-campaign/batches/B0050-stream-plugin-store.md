# B0050 — Stream Plugin Store

## Scope

- Baseline: `82d08a26e3b68cb64249c50bc357169bf1cbc40f`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Owned code: `edgion-gateway/src/plugins/stream/stream_plugin_store.rs` and `edgion-gateway/src/plugins/stream/mod.rs`.
- Approved support: `edgion-resources/src/resource/meta/impls.rs`, `skills/01-architecture/02-gateway/13-stream-plugin-dev.md`, `skills/04-review/plugin-runtime/intentional-design/plugin-runtime-god-match.md`, `skills/04-review/tls/regression-guards/tls-route-stage-isolation.md`, and `skills/04-review/plugin-runtime/regression-guards/invalid-stream-geoip-generation-fail-closed.md`.
- Forbidden files: every other source/knowledge file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Exact commit allow-list: the seven technical/knowledge paths plus B0049/B0050 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Analysis verdict: `change`; moderate multi-file cleanup, handled plan-first.
- `compile_stream_plugins` has no production caller and one store-local test caller. It directly builds both runtimes without demanded-stage selection or GatewayClass `pluginPolicy`, duplicating the canonical construction boundary.
- Remove the direct compiler, its facade export, unused import, and only the bypass-specific test fragment. Retain full/delta/delete/disabled-generation store lifecycle coverage.
- Correct the DTO/source metadata and five ownership pages so `StreamPluginStore` forwards source facts, `PluginPublisher` owns atomic publication, and `StreamPluginGeneration` is the sole production executable constructor.
- Preserve scoped policy, demanded-stage selection, entry sharing/release, fail-close behavior, immutable admission pinning, source update semantics, admin stats, and isolated test scopes.
- Do not move `CompiledStreamPlugins`, alter store keys, add observability/cache mechanisms, change schema/transport, or touch frozen integration tests.

## Plan review

- The implementer proposed exact removal of the direct compiler/export/test fragment, import relocation, and seven-file ownership corrections while preserving all publication and lifecycle invariants.
- An independent plan reviewer approved without scope or sequence changes. Wording must call `StreamPluginGeneration` the sole production resource-to-executable-view orchestration path, while retaining the runtime entry constructors and direct unit-test seams.

## Implementation

- Removed `compile_stream_plugins`, its facade export, production-only import, and the bypass-specific unpublished-runtime test fragment while retaining all lifecycle assertions.
- Preserved `CompiledStreamPlugins`, source full/delta/delete behavior, admin inventory, scoped policy/demand selection, entry sharing/release, fail-close behavior, and admission pinning.
- Corrected the resource comment and five ownership pages to the Store source adapter -> Publisher scoped atomic publication -> Generation production resource-to-view orchestration model, without misclassifying low-level runtime constructors or test seams.

## Independent review

- An independent final reviewer approved the seven-file diff with no findings after caller searches, API/test discrimination checks, ownership wording verification, and focused execution.

## Verification

- Focused verification passed: 4 store, 3 generation, 88 stream with one ignored measurement, and 10 TLS route tests.
- Full repository gates passed: formatting, workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only. Only the campaign's known lint and duplicate-dependency warnings remain.
- Final frozen-path verification passed for all 3,442 recorded integration paths.

## Handoff

- Ledger/changelog updated: yes.
- Commit: `fcfe504127d5f73f883ba9e05d660a2ccea36604`.
