# B0100 — RequestAccessPolicy

## Scope
- Baseline: `c46fceab`
- Pipeline state: commit-ready
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/request_access_policy/{mod.rs,plugin.rs}`
- Revalidated blobs: `mod.rs` = `524fb9f4bffad5a5f5adbf9dc2c096b12c0e2dd2`; `plugin.rs` = `4d44aa2ea62301ac8a27f391ede277de838a3362`
- Implementation allow-list: `edgion-gateway/src/plugins/http/request_access_policy/plugin.rs` only
- Forbidden files: `mod.rs`, resources, Controller, runtime/session/context, documentation, Cargo, every existing `edgion-tests/**` path, and protected user paths

## Analysis boundary
- Trace resource validation and runtime-profile preparation, plugin construction, request profile selection, default-deny evaluation, Condition/HMAC/IP/body checks, rejection ownership, logs, tests, integration coverage, and documentation.
- Preserve the canonical allow-only plus `defaultAction` policy shape, fail-close behavior, HMAC and IP security semantics, request body ownership, `ErrResponse`, sensitive-data handling, and access-log ownership.
- Existing integration tests remain frozen. Record a separate issue only if a genuine future integration gap cannot be covered deterministically in colocated tests.

## Handoff
- Analyst: independent read-only analysis completed on `e391354b`; both anchor blobs were unchanged when promoted on `c46fceab`
- Decision: simple ownership optimization; consume the validated runtime-profile generation, borrow selected profile names during each request, and use a static plugin name
- Supporting scope: none
- Deferred: none; D0046 and the existing clone-cheap local-response task retain adjacent ownership
- Implementation: exact one-file allow-list honored; consumed the private prepared profile generation, borrowed active/default selector names within the snapshot guard, removed the owned constant name, and added deterministic static-name coverage
- Implementer verification: 16 Gateway, 17 resources, and 22 Controller RequestAccessPolicy tests; locked Gateway library check, format, no-tracing, English-only, diff, and frozen-path guards passed
- Final review: independently approved with no Must Fix or Should Fix; ownership, validation failures, selector lifetimes and fallbacks, Condition/HMAC/IP/body behavior, failure responses, logs, sensitive-data handling, and frozen scope remain equivalent
- Coverage: `plugin.rs` changed; `mod.rs` examined-no-change
- Verification: 16 Gateway, 17 resources, and 22 Controller RequestAccessPolicy tests passed; format, locked workspace check/Clippy, Gateway binary build, agent-doc, SSA, metrics, Pingora source, no-tracing, English-only, diff, and frozen-path guards passed
- Integration: the frozen `edgion-tests/**` tree remained unchanged; existing static/dynamic policy, Secret/HMAC/IP, active selector, access-log, and hot-reload coverage is sufficient, so no integration issue was created
- Commit: `3e450400`
