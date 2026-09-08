# B0150 — Connection Rate-limit Plugin

## Scope

- Baseline and promotion HEAD: `81cfc7eb`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/stream/connection_rate_limit/plugin.rs`
- Necessary support file: `cicd/checks/check_gateway_no_tracing.py`
- Baseline blob: `78ca0469`
- Exact implementation allow-list: owned and support files only

## Handoff

- Analysis: `b0142_test_helpers_analysis` audited the complete plugin after its B0148 support edit, including resource validation, Controller publication, runtime compilation, every attachment lifecycle, local and Redis admission, key identity, timeouts, failure policies, metrics, IP identity and tests. Decision: change; simple target-local Gateway defense.
- Defect: the shared validator rejects missing dimensions, zero rates, invalid intervals, missing or malformed Redis references, invalid prefixes and invalid timeouts, but the generic Controller processor can still publish an invalid entry after recording status. The Gateway constructor previously skipped validation and could turn invalid state into pass-through, fallback values or default fail-open Redis behavior.
- Implementation: the constructor now calls the shared `PluginValidation` contract once, emits one bounded cold-path error with safe entry identity, and records initialization failure. The connection callback rejects invalid instances with the fixed `connrate:deny:invalid-config` reason before local gates, Redis, metrics or dependency failure policy. Valid instances add one predictable boolean branch. Documentation now distinguishes algorithm steps from stream lifecycle stages.
- Logging boundary: `ConnectionRateLimitPlugin::new` is registered exactly in the central Gateway cold-function registry. The log contains no validation detail, configuration value or secret and is never emitted per connection.
- Preserved contracts: valid local and Redis algorithms, monotonic local time, Redis epoch time, scoped keys, `DirectPeerIp`, Stage ordering, failure policies, metrics and existing denial reasons.
- Deferred: none. D0028 retains numeric bounds, D0099 retains listener accept-loop scheduling and D0100 retains local-gate linearizability and registry contention.
- Review: `b0135_final_review` independently verified the shared validator, Controller continue-publication path, all Gateway consumers and valid/invalid runtime behavior. It required an exact cold-function registration and safe `entry_scope` on the constructor log; the corrected diff received LGTM with no remaining Must Fix or Should Fix.
- Verification: all 5 plugin and 3 stream-runtime tests passed. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and 3,093-file frozen integration guards passed; Clippy retained only existing warnings.
- Commit: `11778889`; local only, never pushed by the campaign.
