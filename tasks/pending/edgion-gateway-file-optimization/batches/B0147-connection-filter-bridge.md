# B0147 — Connection Filter Bridge

## Scope

- Baseline and promotion HEAD: `67d126ed6a38b77d99fb17936986e419936c2680`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/stream/connection_filter_bridge.rs`
- Baseline blob: `6d7d377878ca11a01d8198e21501a2b023dc8e4a`
- Exact implementation allow-list: owned file only

## Handoff

- Analysis: `b0142_test_helpers_analysis` audited listener construction, the fatal PROXY-policy guard, Pingora accept ordering, store snapshots, every Stage 1 plugin, identity semantics, observability and tests. Decision: change; simple documentation-only correction.
- Value: make the pre-PROXY and pre-TLS admission boundary explicit, identify the physical peer as `DirectPeerIp`, document same-namespace bare-name attachment, admission-scoped generation pinning, the narrow no-inet pass-through and why denial collapses to a silent boolean.
- Preserved contracts: every API and runtime byte, fatal-guard-first ordering, store-miss fail-close, serial first-deny semantics, immutable compiled runtimes, configured dependency failure policy and the accepted no-tracing listener-level rejection boundary.
- Deferred: D0099 records high-severity accept-path head-of-line blocking when Pingora's single listener task awaits dependency-backed Stage 1 filters. It requires a plan-first bounded admission design and deterministic slow-filter coverage; existing integration tests remain frozen.
- Implementation: corrected only module, struct and local comments. The final text distinguishes the accept, fatal guard, bridge, PROXY parsing and TLS/HTTP phases; constrains snapshot pinning to one inet-peer admission decision; and avoids inventing request or canonical access-log metadata unavailable to Pingora's boolean filter interface.
- Review: `b0135_final_review` independently traced the full listener and plugin path. Two review rounds corrected overbroad claims about which connections reach the bridge and whether the cloned outer `CompiledStreamPlugins` generation remains pinned beyond the admission decision. The corrected diff received LGTM with no remaining Must Fix or Should Fix. The reviewer independently confirmed D0099 as non-duplicate and high severity.
- Verification: 3 bridge, 2 listener attachment and 12 fatal PROXY-policy tests passed. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and 3,093-file frozen integration guards passed; Clippy retained only existing warnings.
- Commit: `1db7a23e`; local only, never pushed by the campaign.
