# B0126 — Gateway API ExtensionRef and Module Root

## Scope

- Baseline: `da2567094703a53ad928698be10388b450e39e43`
- Pipeline state: committed
- Active anchor crate: `edgion-gateway`
- Candidate owned files:
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs`
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/mod.rs`
- Baseline blobs: `180626d8f9b96691c503f0cd67f6030d023eefbf` and `d34ac8c8f06fe3dd4016b35d16c3029e21a334ef`
- Exact implementation allow-list:
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs`
  - `docs/en/user-guide/http-route/filters/plugin-composition.md`
  - `docs/zh-CN/user-guide/http-route/filters/plugin-composition.md`
- Forbidden files: every file outside an independently justified exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary

- Read the complete ExtensionRef adapter and module façade, route-filter compilation/activation, supported kinds, validation/status behavior, tests, docs, and relevant review knowledge.
- Preserve Gateway API semantics, fail-closed unsupported references, immutable publication, canonical diagnostics/access-log ownership, redaction, observability restraint, and response streaming.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be represented without editing an existing file.

## Handoff

- Analysis: change `extension_ref.rs`; examine `mod.rs` with no change. The rule and backend request/response prepare paths repeatedly format the same lookup key, including rule-scope index lookups that do not consume it. The backend-scope silent body helper would also allocate if dispatched, although production backendRef body dispatch is currently absent and separately tracked. Precomputation removes the real repeated stage allocations and keeps that body helper compliant with its zero-allocation contract. The private prepare result additionally represents an impossible `Ok(None)` fail-open state.
- Implementation direction: compile name, lookup key, and supported-kind flag once into a shared immutable target held by the four stage clones; borrow the precomputed key on hot paths; preserve the infallible constructor and unsupported-kind ExtensionRef-attributed 500/log behavior; make prepare success non-optional. Add a narrow zero-allocation accessor regression without claiming cache insertion is allocation-free.
- Documentation correction: both plugin-composition pages incorrectly say load-balancing ExtensionRefs are consumed by backend resolution. Current Controller/runtime behavior accepts only EdgionPlugins and directs load-balancing policy to EdgionBackendTrafficPolicy.
- Complexity: simple; one private representation/helper cleanup plus mechanically paired documentation, with no schema, protocol, stage, or public behavior change.
- Promotion freshness: current at `da256709`; owned blobs and both documentation blobs match HEAD. The exact documentation blobs are `0650f90c6565997073241880847a25f66f7de858` and `0bf8e2c42a414089c448340fd582259f7ed1f053`.
- Deferred ownership: [`D0089`](../deferred/D0089-fail-closed-missing-extensionref-configuration.md) records the independently confirmed non-Kubernetes fail-open when `type: ExtensionRef` omits its optional payload. Its coordinated shared-schema, HTTP/GRPC Controller status, Gateway backstop, and tests remain outside B0126; no integration issue is needed now.
- Implementation: introduced one shared immutable compiled target containing the name, lookup key, and preserved supported-kind decision; stage clones share it through `Arc`, lookup borrows the precomputed key, and prepare success is non-optional. Added a narrow zero-allocation accessor regression and corrected both composition guides without changing failure or stage behavior.
- Independent final review: LGTM with no Must Fix. The reviewer confirmed immutable sharing, exact support predicate, infallible construction, rule/backend lookup ownership, failure/log attribution, WAF/dye/identity/streaming behavior, meaningful allocation coverage, paired docs, and `mod.rs` examined-no-change. It also confirmed D0089 as high-severity, nonduplicate, and correctly scoped.
- Verification: 7 ExtensionRef, 36 PluginRuntime, 5 ExtensionRef-index, and 1 named smoke test passed; `cargo fmt --all`, `cargo check --locked --workspace --all-targets`, `cargo clippy --locked --workspace --all-targets`, and the locked Gateway binary build passed. Agent-doc, SSA, metrics-inventory, Pingora-source, Gateway no-tracing, English-only, diff, and frozen-path guards passed.
- Commit: `2cae2cfc`.
