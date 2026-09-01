# B0103 — RequestRestriction

## Scope
- Baseline: `10b0584d`
- Pipeline state: commit-ready
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/request_restriction/{mod.rs,plugin.rs}`
- Revalidated blobs: `mod.rs` = `8564961f95cfb73ddf10f5f07bc9ff3f48d80c03`; `plugin.rs` = `31822efbd94de1f4c2faeabfaa53dbdac96936df`
- Implementation allow-list: `edgion-gateway/src/plugins/http/request_restriction/plugin.rs` only
- Forbidden files: `mod.rs`, resources/schema, Controller, PluginRuntime, shared conditions/HMAC/IP, session/body gate, documentation, tasks, Cargo, every existing `edgion-tests/**` path, and protected user paths

## Analysis boundary
- Traced schema and Controller reference resolution, PluginRuntime construction, prepared Condition ownership, request polarity and short-circuiting, KeyGet/path/IP/Secret/HMAC/body evaluation, response writing and termination, logs, tests, integration coverage, and documentation.
- Preserve Deny/Allow polarity and order, unresolved candidate semantics, descriptor publication, canonical KeyGet/path and DirectPeerIp/RemoteIp ownership, one BodyStorage read, fixed internal-error responses, operator message bytes, header/body/shutdown order, `ErrTerminateRequest`, and access-log diagnostics.
- Existing integration tests remain frozen.

## Handoff
- Analyst: independent read-only analysis completed on `6537bb33`; both target blobs were unchanged when promoted on `10b0584d`
- Decision: simple/medium single-file ownership optimization; retain only prepared scalar/response policy plus the compiled conditions, precompute denial bytes, use a static plugin name, and discard the duplicated full config and unused validation detail
- Implementation: exact one-file allow-list honored; prepared the validated policy and denial bytes once per generation, removed duplicated config/error/name state and request-log formatting allocation, and added deterministic ownership, byte-reuse, invalid-config, and static-name coverage
- Implementer verification: 17 Gateway RequestRestriction, 18 resources RequestRestriction, 44 shared-condition, and 2 PluginRuntime body-requirement tests; locked Gateway all-target check, format, no-tracing, English-only, diff, and frozen-path guards passed
- Final-review correction: production behavior approved; one Should Fix requires the byte-reuse test to retain both returned `Bytes` values before comparing storage identity so allocator address reuse cannot cause a false positive
- Correction implementation: the exact one-file scope was retained; both response `Bytes` now remain alive through the final pointer comparison, eliminating allocator-reuse false positives without changing production code
- Final review: independently approved after correction with no remaining Must Fix or Should Fix; prepared ownership, validation and unresolved-reference fail-close behavior, polarity/order, condition/body/HMAC/IP semantics, response bytes/order/termination, logs, and sensitive-data boundaries remain correct
- Coverage: `plugin.rs` changed; `mod.rs` examined-no-change
- Verification: 17 Gateway RequestRestriction, 18 resources RequestRestriction, 44 shared-condition, and 2 PluginRuntime body-requirement tests passed; format, locked workspace check/Clippy, Gateway binary build, agent-doc, SSA, metrics, Pingora source, no-tracing, English-only, diff, and frozen-path guards passed
- No-change candidate: `mod.rs`
- Deferred: D0046 retains invalid/final HTTP status unification; shared compiled-predicate ownership remains a cross-consumer boundary and is not changed here
- Integration: existing frozen coverage is sufficient; generation ownership and byte reuse are deterministic unit-test concerns, so no integration issue is needed
- Commit: `2e744ada`
