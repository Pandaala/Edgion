# B0063 — AllEndpointStatus Correctness and Hot-path State

## Scope
- Baseline: `1954449161fae47cce027aacb978ed680695d68c`
- Pipeline state: reviewed and fully verified; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/all_endpoint_status/{mod.rs,plugin.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/all_endpoint_status/plugin.rs`
- Expected no-change file: `edgion-gateway/src/plugins/http/all_endpoint_status/mod.rs`
- Allowed tests: colocated tests in the implementation file only
- Forbidden files: every existing `edgion-tests/**` path, resource schemas, shared outbound clients and endpoint stores, Controller code, metrics/access-log code, other plugins, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: the implementation file; campaign ledger/changelog/main record; B0062 and B0063 batch records; reviewer-approved deferred task records or precise existing-task clarifications plus campaign backlinks; `deferred/INDEX.md`

## Context and value gate
- Analysis subagent: `b0055_runtime_analysis`, current at `19544491`; both target files are clean.
- Baseline: Gateway AllEndpointStatus tests pass 3/3 and shared resource tests pass 14/14.
- Reference authorization bug: the plugin accepts Service backend references without checking the Controller-resolved `ref_denied` marker, then resolves EndpointSlices and dials them. A denied cross-namespace Service reference can bypass the canonical backend authorization gate.
- Result identity bug: asynchronous probe results are keyed only by address. Multiple backend occurrences may legitimately share one address, causing overwrite, false wall-timeout synthesis, and lost results.
- Summary contract bug: any HTTP response is counted as success although the public contract defines only 2xx/3xx as success; body-byte accounting should include every response actually read.
- Configurable panic: an unbounded `concurrencyLimit` reaches `tokio::sync::Semaphore::new`, even though the actual endpoint set is capped and permits above that size add no value.
- First-request and hot-path state bug: zero is both the global limiter's uninitialized sentinel and a valid startup timestamp, which can reject initial requests; an immutable once-initialized process configuration is nevertheless read behind an RwLock on each request.
- Expected no-change file: `mod.rs` retains the minimal private-module and single re-export facade.
- Preserved behavior: canonical shared outbound client and egress enforcement, redirect/proxy policy, header filtering, endpoint cap, wall timeout and partial results, fixed plugin rejection bodies, count-only diagnostics, response schema, metrics, access-log ownership, and request-body streaming rules.

## Deferred candidates
- BackendTLSPolicy-aware endpoint probes require a complete outbound TLS identity, CA, mTLS, SAN, connection-pool generation, hot-update, and fail-close contract. The existing active-health-check TLS task is adjacent but does not currently own this plugin.
- Service port-to-EndpointSlice serving-port mapping is already owned by the repository-wide backend-port-resolution task; AllEndpointStatus should be named as a consumer rather than receive a duplicate local fix.
- Several closed review records still describe obsolete AllEndpointStatus header-forwarding behavior or a missing todo; this is knowledge maintenance, not a reason to change runtime code in B0063.
- Existing integration tests remain frozen. The local corrections have deterministic unit-test seams and do not require a new integration issue; any future real TLS coverage belongs to the approved TLS design task.

## Plan-first implementation
- Implementer: `b0056_implementation`
- Complexity: complex; combines reference authorization, bounded fan-out result identity, summary semantics, panic prevention, and process-wide time/config state
- Required plan review: approved independently by `b0056_final_review` after two plan corrections
- Approved plan: publish immutable global config only through the startup initializer while the getter falls back to an independent default; encode the execution clock so zero means unclaimed; filter denied references before resolution; retain full discovery counts and an ordered globally selected prefix; fan out by bounded occurrence ordinal; assemble stable slots; classify only 2xx/3xx as success and count every received response body's raw bytes.
- Test contract: use only local atomics/config state; cover epoch zero, exact interval, rollback, contention, saturation and older completion; denied-before-resolver; no discovery, zero cap, and prefix truncation; duplicate addresses, out-of-order and missing results; huge permits, empty fan-out, exact deadline; response status and invalid-UTF-8 raw-byte accounting.
- Actual change: the immutable global config is now published once without request-path locking; the execution gate uses a nonzero encoded clock with CAS and monotonic completion; denied references are excluded before resolution; full discovery accounting is separated from the selected ordered prefix; bounded ordinal fan-out preserves duplicate addresses and exact-deadline completions; summaries use 2xx/3xx success and all received raw body bytes.

## Independent review
- Plan reviewer: `b0056_final_review`
- Code reviewer: `b0063_fresh_code_review`, distinct from the plan reviewer
- Plan verdict: Approved after preventing default getter reads from occupying the OnceLock and preserving pre-truncation accounting when the effective endpoint cap is zero
- Code verdict: Approved with no Must or Should findings after a fresh source-first review; 14 Gateway and 14 resource tests passed, and the Gateway suite passed five repeated runs.
- Deferred adjudication: D0032 owns the nonduplicate on-demand BackendTLSPolicy probe contract; the existing backend-port task now names AllEndpointStatus; D0033 owns stale review-knowledge corrections. Existing integration tests remain frozen, and no new B0063 integration issue is warranted.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | user-authorized concurrent baseline unchanged |
| Baseline Gateway tests | passed | 3 tests |
| Baseline resource tests | passed | 14 tests |
| Implementation and focused tests | passed | 14 Gateway and 14 resource tests; saturation, deadline, and concurrency regressions repeated during implementation/review |
| Gateway all-target compile | passed | implementation and correction verification |
| Independent code review | passed | fresh reviewer distinct from the plan reviewer; no Must or Should findings |
| Full campaign gates | passed | format, workspace check, workspace Clippy, agent docs, SSA, metrics inventory, Pingora source, and gateway no-tracing |
| Final frozen integration paths | passed | existing integration tests unchanged from the user-authorized baseline |

## Handoff
- Ledger/changelog updated: yes; one changed and one examined-no-change
- Commit: pending
