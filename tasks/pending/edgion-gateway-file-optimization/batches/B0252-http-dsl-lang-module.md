# B0252 — HTTP DSL Lang Module

## Scope

- Pipeline state: analysis-ready; examined-no-change candidate awaiting serial promotion and independent review.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `f326f2800c03c34fce39f9fbedf73f18cacc2dc1`.
- Target: `edgion-gateway/src/plugins/http/dsl/lang/mod.rs`.
- Target blob: `216eecfa7ed4cd6259648dfb77f6290fe08ce682`.
- Read scope includes the complete Gateway DSL language/runtime module and every facade consumer/export.
- Frozen write scope is not yet authorized; analysis is read-only.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: examined-no-change.
- The facade contains no executable logic: it declares Gateway-owned builtin/VM runtime modules and provides locality aliases to the runtime-independent `edgion-dsl` leaf. Its runtime cost is zero.
- Active bytecode/error/validator/value aliases support nearby VM, builtin, plugin and test imports. Removing unused-in-repo ast/compiler/parser aliases, switching every consumer to leaf paths or narrowing visibility would be nominal API tidiness; workspace policy explicitly rejected broad `unreachable_pub` churn.
- Re-exporting newer leaf-only helpers would broaden the wrong ownership layer. No correctness, security, observability, performance, deferred or integration issue was found.
- Baseline remains `f326f280`; target blob `216eecfa7ed4cd6259648dfb77f6290fe08ce682`.
