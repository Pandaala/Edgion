# B0153 — Global Connection IP Restriction Facade

## Scope

- Baseline and promotion HEAD: `1ceed397526a3665a8926b2a09d260afd6b65d34`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/stream/global_connection_ip_restriction/mod.rs`
- Baseline blob: `bc2de2d228b757d4b373410c24efb7214c56d493`
- Exact implementation allow-list: owned file only

## Handoff

- Analysis: `b0142_test_helpers_analysis` traced the facade, reexports, runtime construction, B0123 implementation and all listener/TCP/UDP/TLS call sites. Decision: change; trivial documentation-only correction.
- Defect: the previous one-line description called the plugin a pre-TLS filter referencing a PM. The implementation owns inline compiled named profiles, may select one through `EdgionConfigData`, and runs at listener or route-owned stream lifecycle points.
- Implementation: corrected only module Rustdoc and stated the invariant that evaluation always uses `DirectPeerIp`. Declarations, public reexports, API and runtime bytes are unchanged.
- Preserved contracts: B0123 generation-consistent Selector and referenced-list snapshot, fail-to-base behavior, validation, hot path, logging and metric ownership.
- Deferred: none. The adjacent implementation was already terminalized by B0123 and is not re-claimed here.
- Review: `b0135_final_review` independently verified profile construction, Selector resolution, snapshot ownership, every lifecycle caller and physical-peer identity. No Must Fix or Should Fix.
- Verification: no facade-specific unit test adds signal beyond compilation. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and 3,093-file frozen integration guards passed; Clippy retained only existing warnings.
- Commit: `eedf452d`; local only, never pushed by the campaign.
