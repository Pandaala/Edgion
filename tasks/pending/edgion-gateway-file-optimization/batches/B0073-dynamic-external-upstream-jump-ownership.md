# B0073 — DynamicExternalUpstream Jump Ownership and Allocation Cleanup

## Scope
- Baseline: `65c02ff0`
- Pipeline state: implementation independently approved, fully verified, and committed as `46e3ec98`
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/dynamic_external_upstream/{mod.rs,plugin.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/dynamic_external_upstream/plugin.rs`
- Expected no-change file: `edgion-gateway/src/plugins/http/dynamic_external_upstream/mod.rs`
- Allowed tests: colocated tests in the implementation file only
- Forbidden files: every existing `edgion-tests/**` path, shared resource/config and docs, PluginRuntime/PluginStore, jump/session/context APIs, shared DNS/TLS/address classification, metrics/access-log implementation, Cargo manifests, other plugins, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: `edgion-gateway/src/plugins/http/dynamic_external_upstream/plugin.rs`; `tasks/pending/edgion-gateway-file-optimization/03-subtasks.md`; `tasks/pending/edgion-gateway-file-optimization/07-changelog.md`; `tasks/pending/edgion-gateway-file-optimization/edgion-gateway-file-optimization.md`; `tasks/pending/edgion-gateway-file-optimization/batches/B0072-direct-endpoint-safe-resolution.md`; `tasks/pending/edgion-gateway-file-optimization/batches/B0073-dynamic-external-upstream-jump-ownership.md`; `tasks/pending/edgion-gateway-file-optimization/deferred/D0045-dynamic-external-upstream-sni-host-validation.md`; `tasks/pending/edgion-gateway-file-optimization/deferred/INDEX.md`; and `tasks/todo/reject-unsafe-dynamic-external-upstream-sni-host-overrides.md`

## Context and value gate
- Analysis subagent: `b0056_implementation`, revalidated against `65c02ff0`; target blobs are `cfa22bf763f350ec4b4d5e69bec376b04b86d462` for `mod.rs` and `023039239106d4cd694728b1ded9f41625f1fd45` for `plugin.rs`.
- Baseline: 4 Gateway DynamicExternalUpstream, 6 resource validation, and 14 canonical address-classification tests pass.
- The plugin applies `overrideHost` before competing for the unified jump slot. A rejected losing jump can therefore overwrite the Host header used by the winning upstream target.
- A successful regex capture is copied solely for a borrowed domain-map lookup; the constant plugin name is allocated per instance; and the Dye `domain:port` string is formatted even when jump ownership is rejected.
- Decision: acquire jump ownership before target-specific Host mutation, retain the routing key as a borrow, format success diagnostics only after `Set`, and return a static plugin name.
- Expected no-change file: `mod.rs` remains the minimal module/export facade.

## Hard-rule boundary
- Preserve request-driven selection only among operator-configured domain-map entries, canonical deferred DNS resolution and resolved-address classification, mapped-IPv6 handling, ExternalJumpPreset TLS/SNI/port/path/timeout/retry fields, and immutable hot reload.
- Preserve gRPC skip, raw-missing/regex-miss/no-map behavior, onMissing fallback/reject policy, fixed response body and Dye causes, path normalization, and first-write-wins semantics.
- On `Set`, apply the existing best-effort `overrideHost`, format the existing `domain:port` Dye value, record Success, and retain the existing `upstream selected` log.
- On `Rejected`, perform no Host mutation, Dye publication, or target formatting; retain exactly `jump locked, rejected`.
- Add no tracing, metrics, access-log fields, schema behavior, public APIs, DNS/TLS/address policy, dependencies, integration-test edits, or duplicate routing mechanisms.

## Deferred candidates
- D0045 is `needs-implementation`: with `tls: true`, shared validation accepts explicit empty SNI that reaches the supported Pingora BoringSSL path and disables verification. Header-value-invalid Host overrides fail silently at request time, while authority-invalid but header-value-valid overrides may be forwarded. This is separate resource validation and bilingual documentation work.
- D0001 owns ServiceExternalName cache identity and lifecycle and is not a duplicate. Regex DFA limits remain an accepted precedent.
- Existing frozen integration coverage has 12 cases for match, fallback/reject, regex, Dye, override configuration, and resolved loopback rejection. The ownership regression is deterministic in colocated mocks, so no new integration issue is required.

## Implementation
- Complexity: simple; one local ordering/ownership fix plus private allocation removal with no public contract change
- Implementer: `b0055_runtime_analysis`; distinct from analyst and final reviewer
- Exact change: removed the allocated constant name, borrowed regex capture keys, acquired the jump slot before Host mutation, and limited target formatting, Host override, Success Dye, and selection logging to accepted ownership
- Test contract: static identity; borrowed regex selection; accepted preset and post-ownership Host mutation; rejected jump with no Host/Dye side effects and only the locked log; preserved missing and fallback behavior

## Independent review
- Code reviewer: `b0073_final_code_review`; distinct from analyst and implementer
- Code verdict: approved with no must-fix or should-fix findings; confirmed borrow lifetime, synchronous ownership order, retry persistence, preset fields, failure policy, and zero rejected-jump side effects
- Deferred adjudication: D0045 `needs-implementation`; empty explicit SNI with `tls: true` disables verification in the supported connector path, while shared validation is the correct owner; no integration issue

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | user-authorized concurrent baseline unchanged |
| Baseline focused tests | passed | 4 Gateway plugin, 6 resource, and 14 canonical network tests |
| Implementation and focused tests | passed | 6 Gateway plugin, 6 resource validation, and 14 canonical network tests |
| Gateway all-target compile | passed | implementation and reviewer runs passed |
| Independent code review | passed | no must-fix or should-fix findings |
| Full campaign gates | passed | format, workspace check, Clippy, agent docs, SSA, metrics, Pingora source, and no-tracing |
| Final frozen integration paths | passed | immutable integration-test baseline unchanged |

## Handoff
- Ledger/changelog updated: yes; 1 changed and 1 examined-no-change row, all verification complete
- Commit: `46e3ec98`
