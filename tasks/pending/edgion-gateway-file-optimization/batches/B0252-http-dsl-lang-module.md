# B0252 — HTTP DSL Lang Module

## Scope

- Pipeline state: terminal; changed; implementation and independent re-review passed.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `397d27eff4aabc68df6610bf68ee37e9f4d7bd1e`.
- Target: `edgion-gateway/src/plugins/http/dsl/lang/mod.rs`.
- Target blob: `216eecfa7ed4cd6259648dfb77f6290fe08ce682`.
- Read scope includes the complete Gateway DSL language/runtime module and every facade consumer/export.
- Frozen write scope is not yet authorized; analysis is read-only.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Initial decision: examined-no-change; independently rejected.
- The facade contains no executable logic: it declares Gateway-owned builtin/VM runtime modules and provides locality aliases to the runtime-independent `edgion-dsl` leaf. Its runtime cost is zero.
- Active bytecode/error/validator/value aliases support nearby VM, builtin, plugin and test imports. Removing unused-in-repo ast/compiler/parser aliases, switching every consumer to leaf paths or narrowing visibility would be nominal API tidiness; workspace policy explicitly rejected broad `unreachable_pub` churn.
- Re-exporting newer leaf-only helpers would broaden the wrong ownership layer. No runtime, security, observability, performance, deferred or integration issue was found.
- Refresh at `397d27ef`: target blob remains `216eecfa7ed4cd6259648dfb77f6290fe08ce682`. B0251 changed the builtin implementation and tests, retained the `lang::builtins`/`lang::value` locality paths, and did not alter this facade's ownership or value assessment.
- Live aliases remain `bytecode`, `error`, `validator`, and `value`; `vm` and `builtins` remain Gateway-owned. The initial analyst proposed retaining unused `ast`/`compiler`/`parser` aliases as a coherent locality surface.

## Independent Review

- The reviewer confirmed the import graph, zero runtime cost, and absence of integration impact, but rejected the no-change disposition as a project hard-rule violation.
- `ast`, `compiler`, and `parser` have zero workspace consumers, canonically belong to `edgion-dsl`, and are explicitly documented as preserving historical Gateway paths. The pre-release rule forbids keeping compatibility re-exports without a current consumer.
- Required change: remove only those three aliases and replace historical/compatibility rustdoc with current runtime-locality ownership. Retain `bytecode`, `error`, `validator`, `value`, `builtins`, and `vm`. No behavioral test or deferred issue is warranted.

## Implementation and Verification

- Removed only the zero-consumer `ast`, `compiler`, and `parser` aliases. Retained all live leaf aliases and Gateway-owned runtime modules.
- Replaced the compatibility rationale with current ownership documentation. Workspace-wide search confirmed no hidden consumer of the removed paths; the standalone Redis integration imports only retained aliases.
- Independent re-review returned LGTM with no remaining finding. `cargo check -p edgion-gateway --all-targets`, formatting, diff, and frozen-integration checks passed; no behavioral or integration test was warranted for this compile-time facade cleanup.
- Commit: `9d3d0354`; never pushed by the campaign.
