# B0154 — Stream IP Restriction Facade

## Scope

- Baseline and promotion HEAD: `eedf452d04ef6b497d368addb20f6296bd079b2e`
- Pipeline state: terminal; examined-no-change
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/stream/ip_restriction/mod.rs`
- Baseline blob: `fcd41dc7c7afdf7cf8ceb7b3cb6166ac25b0c198`
- Exact implementation allow-list: none

## Handoff

- Analysis: `b0142_test_helpers_analysis` traced the crate-private child declaration, public type reexport, parent facade, runtime consumer, B0123 implementation and B0082 HTTP precedent. Decision: examined-no-change.
- Retained value: the facade keeps the implementation path inside the Gateway crate while exposing the sole `StreamIpRestriction` type required by runtime construction. It adds no runtime cost and contains no stale documentation.
- Rejected churn: making the child private or public, moving the live reexport, or duplicating the adjacent implementation's matcher, snapshot, validation and DirectPeerIp contracts adds no security, performance or ownership value.
- Coverage boundary: the adjacent implementation was terminalized by B0123 and was inspected only to validate the facade.
- Deferred: none.
- Review: `b0135_final_review` independently confirmed the visibility boundary, reexport chain, only production consumer, HTTP precedent and absence of duplicate mechanisms or stale claims. No Must Fix or Should Fix.
- Verification: no facade-specific test adds signal beyond compilation. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and 3,093-file frozen integration guards passed; Clippy retained only existing warnings.
- Commit: `9e6b23e7`; local only, never pushed by the campaign.
