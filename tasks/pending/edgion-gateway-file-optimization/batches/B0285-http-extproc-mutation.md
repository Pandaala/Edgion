# B0285 — HTTP ExtProc Mutation

- Pipeline state: analysis-ready; change candidate, complex / plan-first.
- Active anchor: `edgion-gateway`; baseline `bd84aa15`.
- Target: `edgion-gateway/src/plugins/http/ext_proc/mutation.rs`.
- Analyst: `b0285_extproc_mutation_analysis`; full mutation/consumer, boundary and coverage/value analysis assigned.
- B0283 owns serial plan review; B0284 is queued ahead. No implementation or review promotion before prior commits and freshness checks.
- Integrations and user handoff/WAF files remain protected; no technical mutation authorized.
- Target blob `850ac44c51637a5b2e0e6a654a34300743887205`; complete collector/mutator and consumers reviewed. get_path omits the query in ExtProc :path; canonical get_uri uses current normalized path and raw query, preserving prior rewrites.
- Proposed scope: mutation.rs plus only the plugin.rs expect_empty_request_headers mock expectation adaptation. Cover query-free and duplicate headers, duplicate query keys, percent-encoded separators, plus and empty query with contrasting current/frozen views. Run only the deterministic mutation test module; no live plugin tests.
- Separate candidate finding: text-only collection, lossy raw-value decoding and text-based existence checks lose opaque header fidelity. Requires its own end-to-end contract task, not part of the query fix. D0149 already covers stale phase knowledge.
- Analyst corrected its initial simple label: protocol-boundary behavior requires complex plan-first regardless the small diff. Revalidate at promotion after B0283/B0284 commits.
