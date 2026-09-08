# B0158 — Stream Plugin Trait

## Scope

- Candidate analysis HEAD: `9e51b72f8b0a2009db4bc713fb04d8e98f95ad25`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/stream/stream_plugin_trait.rs`
- Baseline blob: `899a304bda492d1d680e43cef19552d3a88c222a`
- Exact implementation allow-list: owned file only

## Handoff

- Analysis: `b0131_final_review` read the complete stream module, all five implementations, compilation/publication, listener/TCP/UDP/legacy-TLS callers, sibling TLS-route trait/runtime, tests and precedent. The target blob remained stable at promotion. Decision: change; simple documentation-only correction.
- Value: the defining trait and method currently understate their invocation contexts as only TCP/UDP. Correcting the contract prevents future implementations from assuming a uniformly route-owned lifecycle after surrounding B0155/B0157 documentation was corrected.
- Implementation brief: document listener admission, TCP/UDP route-owned lifecycle execution, legacy TLSRoute compatibility fallback, and the separate `TlsRoutePlugin` contract for new TLS-route configuration. Use lifecycle-neutral method wording. Preserve all signatures, types, result reasons, ordering, async execution, labels, IP semantics, logging and runtime behavior.
- Verification: focused stable-label test, formatting, exact diff containment and the complete campaign gate.
- Integration coverage: no gap; this is a documentation-only correction and existing integration files remain frozen.
- Implementation: corrected only the trait and method Rustdoc. Every type, signature, implementation, result, ordering rule, async boundary, label, IP field and runtime behavior is unchanged.
- Review: `b0135_final_review` independently traced all listener, TCP, UDP and legacy TLS callers and returned LGTM with no Must Fix, Should Fix or deferred finding.
- Verification result: focused stable-label test 1/1; `cargo fmt --all`; locked workspace all-target check and Clippy; locked Gateway binary build; agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff and frozen-path guards all passed. Clippy retained only existing warnings.
- Commit: `ea8dd592`; never pushed by the campaign.
