# B0069 — CredentialInjector Sensitive Buffer Transfer

## Scope
- Baseline: `c3c05d16`
- Pipeline state: independently reviewed and fully verified; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/credential_injector/{mod.rs,plugin.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/credential_injector/plugin.rs`
- Expected no-change file: `edgion-gateway/src/plugins/http/credential_injector/mod.rs`
- Allowed tests: colocated tests in the implementation file only
- Forbidden files: every existing `edgion-tests/**` path, resource and Controller credential resolution, session/context and upstream filter code, plugin runtime/framework code, metrics/access-log code, docs, Cargo manifests, other plugins, campaign deferred records, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: `edgion-gateway/src/plugins/http/credential_injector/plugin.rs`; `tasks/pending/edgion-gateway-file-optimization/03-subtasks.md`; `tasks/pending/edgion-gateway-file-optimization/07-changelog.md`; `tasks/pending/edgion-gateway-file-optimization/edgion-gateway-file-optimization.md`; `tasks/pending/edgion-gateway-file-optimization/batches/B0068-cors-bounded-header-assembly.md`; and `tasks/pending/edgion-gateway-file-optimization/batches/B0069-credential-injector-sensitive-buffer-transfer.md`

## Context and value gate
- Analysis subagent: `b0056_implementation`, revalidated against `c3c05d16`; both target blobs and the Secret/LinkSys resolution, conf-sync, construction, deferred request mutation, attempt-local insertion, retry, and hot-reload call chains remain unchanged.
- Baseline: 6 Gateway CredentialInjector tests, 3 deferred-credential context tests, 2 upstream Connection tests, 8 resource tests, and 14 Controller tests pass.
- Construction currently builds a complete credential String and then copies the same sensitive bytes into `HeaderValue::from_str`, temporarily retaining an avoidable extra full-value allocation.
- The pinned `bytes` and `http` versions support `BytesMut::freeze` followed by `HeaderValue::from_maybe_shared`, transferring the same allocation without copying.
- Decision: assemble the bounded credential in `BytesMut`, transfer it into `HeaderValue`, and immediately preserve the required sensitive flag. Keep all subsequent Arc/generation ownership unchanged.
- Expected no-change file: `mod.rs` remains the minimal facade.

## Hard-rule boundary
- Preserve the 16 KiB credential bound, Bearer/custom prefixes and exact bytes, invalid-header failure, fixed 500 fail-close behavior, static PluginLog/error content, deadline and Connection checks, client-header replacement, attempt-local injection, retry generation pinning, mirror isolation, and Secret update/delete semantics.
- Every constructed credential `HeaderValue` must call `set_sensitive(true)` before storage or cloning so Debug masking and HTTP/2 never-indexed encoding remain intact.
- This batch removes one local transient copy only; it does not claim zeroization or an end-to-end credential lifetime guarantee. D0036 remains the canonical cross-crate audit.
- Add no tracing, metrics, access-log fields, public APIs, dependencies, integration-test edits, or duplicate deferred tasks.

## Implementation
- Complexity: simple; one private bounded construction representation change with no control-flow or public-contract change
- Implementer: `b0055_runtime_analysis`, distinct from the analyst and final reviewer
- Exact change: assembled the bounded credential bytes in `BytesMut`, froze and transferred that allocation through `HeaderValue::from_maybe_shared`, and marked the resulting value sensitive before Arc storage.
- Test contract: exercise the production constructor helper and assert allocation identity across buffer transfer, exact credential bytes, the sensitive flag, and masked Debug output

## Independent review
- Code reviewer: `b0063_fresh_code_review`, distinct from the analyst and implementer
- Code verdict: approved with no findings after verifying the checked byte bound, current dependency transfer path, equivalent header validation, immediate sensitivity marking, deterministic pointer test, and unchanged injection/lifecycle behavior.
- Deferred adjudication: D0036 already owns the remaining Controller/conf-sync/runtime/plugin credential-copy and lifetime audit. B0069 needs no new integration issue because allocation identity is a deterministic unit-level invariant and frozen integration scenarios already cover injection, ordering, isolation, failure, hot reload, and deletion.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | user-authorized concurrent baseline unchanged |
| Baseline focused tests | passed | 6 Gateway, 3 deferred-context, 2 upstream Connection, 8 resource, and 14 Controller tests |
| Implementation and focused tests | passed | 7 Gateway, 3 deferred-context, 2 upstream Connection, 8 resource, and 14 Controller tests; pointer transfer passed twenty repeated runs |
| Gateway all-target compile | passed | implementation and independent review verification |
| Independent code review | passed | fresh reviewer distinct from analyst and implementer; no findings |
| Full campaign gates | passed | format, workspace check, Clippy, agent docs, SSA, metrics, Pingora source, and gateway no-tracing |
| Final frozen integration paths | passed | manifest unchanged; no existing integration test edited |

## Handoff
- Ledger/changelog updated: yes; one changed and one examined-no-change
- Commit: `71312d4e`
