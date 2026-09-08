# B0155 — Stream Facade

## Scope

- Analysis HEAD: `9e6b23e70c61225c071b976673c6f8fcd9a07893`; promotion HEAD after external update: `aee7a7de`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/stream/mod.rs`
- Baseline blob: `bb125bf6429652669149bcd7befe20788b487ce1`
- Exact implementation allow-list: owned file only

## Handoff

- Analysis: `b0142_test_helpers_analysis` audited all declarations and reexports plus listener, TCP, UDP, terminated TLS and passthrough TLS consumers. The target baseline stayed stable across the external HEAD update. Decision: change; simple Rustdoc-only correction.
- Defect: the previous top-level docs described a two-stage architecture and equated Stage 1 with a pre-TLS, IP-only ConnectionFilter. The same connection-context runtime also runs at TCP/UDP route session creation and through an explicitly legacy TLSRoute compatibility fallback, while connection rate limiting may use Redis.
- Implementation: documentation now distinguishes `StreamPluginRuntime`, which consumes `DirectPeerIp` and listener port at listener/TCP/UDP/legacy fallback points, from `TlsRoutePluginRuntime`, which runs after TLSRoute matching with terminated post-handshake context or inspected passthrough ClientHello/SNI context before forwarding the end-to-end handshake. Declarations, reexports, API and runtime are unchanged.
- Preserved contracts: the legacy fallback remains discouraged for new configuration; no visibility, logging, redaction, failure policy, hot-path or object-safety behavior changed.
- Deferred: none.
- Review: `b0135_final_review` independently traced both runtime families and required correcting an overbroad post-handshake claim for TLS passthrough. The corrected diff received LGTM with no remaining Must Fix or Should Fix.
- Verification: no behavior test is required for a Rustdoc-only facade correction. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and 3,093-file frozen integration guards passed; Clippy retained only existing warnings.
- Commit: `d9efd857`; local only, never pushed by the campaign.
