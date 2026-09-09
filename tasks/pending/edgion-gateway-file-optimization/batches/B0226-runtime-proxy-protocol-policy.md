# B0226 — Runtime PROXY Protocol Policy

## Scope

- Pipeline state: analysis-ready; queued, not active mutation.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/runtime/server/proxy_protocol_policy.rs`.
- Baseline: `4c575bee`.
- Analyst: `b0226_proxy_policy_analysis`.
- Inspect full policy ownership, startup/publication/readiness/shutdown callers and transport trust boundaries; the file was added by external commit `8cbbed6c` and has not received full campaign coverage.
- No writes authorized. Existing integration tests, external todo repairs and user-owned handoff/WAF paths remain protected; new integration needs are deferred.

## Resume Note

- Initial analyst read the target/rules and began startup/publication/fork trust-order checks, then the provider returned a model-capacity error before a final report. No coverage conclusion, source change or verification is claimed. Resume this analysis when a worker slot is available; it does not block the serial B0224 plan-review gate.

## Completed Analysis

- Original analyst resumed successfully at `3786c2cd`; recommends examined-no-change. Target blob `dbf839e18ba3bb6a844f78233d3db1e51f09857f` unchanged from `4c575bee`; source/issue allow-list empty, no Cargo run.
- Full policy, CLI/cache/handler/preflight/readiness/shutdown and installed fork trust chain verified. Hot accept uses an atomic fatal load then existing dispatch; trust uses canonical radix matching. Clones/sets/protocol normalization are cold configuration costs without demonstrated material benefit.
- Typed canonical CIDRs and shared listener contributors fail closed on invalid/conflicting transported policy; preflight freezes under mutex and later semantic divergence trips a monotonic guard. Disabled absence equivalence, contributor identity versus socket semantics and actual serialized dispatch are preserved.
- TrustedSources is intentionally opportunistic, not Required mode; physical peer trust precedes probing and DirectPeerIp stays separate from resolved identity. Fatal guard stops newly checked connections, not atomically all already accepted work. No stronger guarantee or defect inferred.
- Retain 12 target tests and frozen lifecycle coverage; no new integration requirement. Fresh filters: `runtime::server::proxy_protocol_policy::tests`, `runtime::server::listener_builder::proxy_protocol_trust_tests`, `cli::pingora::tests`.
- Original knowledge: Gateway startup/shutdown, listeners `compatibility-constraints/listener-port-set-frozen-at-startup.md`, log-safety/Rust/unit-test rules. No knowledge correction required.
