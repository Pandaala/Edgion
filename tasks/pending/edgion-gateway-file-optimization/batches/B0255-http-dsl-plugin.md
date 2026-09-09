# B0255 — HTTP DSL Plugin

## Scope

- Pipeline state: in-progress; refreshed analysis passed the value gate after B0254.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `53b436aa`.
- Target: `edgion-gateway/src/plugins/http/dsl/plugin.rs`.
- Target blob: `0f974fc8c59a7cfc935256f56cd8f741fbf8871a`.
- Read scope includes the complete DSL config/plugin/runtime/VM path, resource-owned validation, plugin access-log construction, public documentation, tests, and review precedent.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: change; moderate.
- `DslConfig.name` is operator-controlled and copied into a newly allocated `PluginLog` on every matching request/response. Names longer than the existing 36-byte `CurrentPluginEntryIdentity` slot also silently lose plugin-entry identity.
- The earlier 128-byte candidate was rejected after current-HEAD analysis. Use the canonical existing 36 UTF-8 byte capacity, never truncate, and keep validation resource-owned for Controller/Gateway parity.
- Exact technical allow-list: the resources-layer DSL config, Gateway DSL plugin tests, and the English/Chinese DSL guide README files. Preserve empty-name wording, compile/link error ordering for valid names, standalone factory defense, retain-previous behavior, logging schema, VM/body/failure policy, and frozen integrations.
- Targeted resource boundary tests and standalone request/response factory tests are required. No CRD change is needed because plugin config uses preserve-unknown schema.
- The wider scalar-limit contract remains a separate candidate pending independent confirmation.

## Implementation and Review

- Added the canonical 36-byte name limit to `DslConfig` and applied it through both shared validation entry points. Empty-name wording and valid-name compile/link error ordering remain unchanged.
- Added ASCII and multibyte byte-boundary tests plus standalone request/response Gateway factory coverage and a test invariant tying the resource limit to the runtime identity capacity. Updated both language guides.
- Independent review initially found a public `DslPlugin::new` bypass. The implementer made it private within the frozen target, leaving the two validated public factories as the only constructors; correction re-review cleared the Must Fix and returned LGTM.
- No truncation, compatibility alias, CRD, access-log schema, observability, body, VM, failure-policy, or integration-test change was introduced.
- D0132 owns the independently confirmed DSL scalar-limit/parity contract. D0133 owns the broader malformed bilingual plugin-list example audit.

## Verification

- Resource DSL tests: 13/13 by the implementer; reviewer filter 12/12.
- Gateway DSL plugin tests: 5/5; Gateway targeted compile passed.
- Format and diff checks passed. Coordinator locked workspace check/Clippy plus agent docs, SSA force, metrics inventory, Pingora source policy, Gateway no-tracing, English-only, diff, and frozen-path gates all passed.
- Frozen integration manifest: 3,231 paths unchanged.
- Commit: `efd98773` (`Bound DSL plugin names`); no push.
