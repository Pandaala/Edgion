# B0047 — LinkSys Elasticsearch Provider

## Scope

- Baseline: `9dd08a5e355aa1cfb2835fbec807cf2cae164039`
- Pipeline state: terminal
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/link_sys/providers/elasticsearch/{bulk.rs,client.rs,config_mapping.rs,log_sender.rs,mod.rs,ops.rs}`
- Allowed support files: colocated unit tests in the six target files only; no cross-crate support is authorized before analysis proves necessity
- Forbidden files: every existing `edgion-tests/**` path, unrelated LinkSys providers/runtime/template files, shared schemas or common mechanisms without exact necessity evidence, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact commit allow-list: the six target files; `03-subtasks.md`; `07-changelog.md`; `edgion-gateway-file-optimization.md`; legacy follow-up index; any analyst-verified new deferred record; the B0046 and B0047 batch records

## Context and value gate

- Analysis subagent: `b0047_analysis`
- Coverage summary: reviewed all six provider files, resource validation/mapping, canonical outbound client/TLS/egress construction, runtime publication, DataSender and bulk lifecycle, health, retry/shutdown paths, tests, sibling evidence, and review precedent.
- Key responsibility/call chain: validated LinkSys configuration creates an `EsLinkClient`; initialization starts health and bounded bulk workers; access-log records enter through a non-blocking sender and are serialized, batched, retried, and drained in the background.
- Concrete opportunity and decisive evidence: `flushInterval: 0` is accepted by schema validation, maps to `Duration::ZERO`, and reaches `tokio::time::interval`, which panics and kills the registered bulk worker.
- Expected benefit: prevent an accepted configuration value from permanently disabling Elasticsearch log delivery.
- Regression surface: only explicit zero changes, becoming a 1 ms interval; positive values, the 60-second ceiling, authentication, TLS, egress, redirects, health, queue, retry, and shutdown semantics remain unchanged.
- Decision: change; item-level bulk retry requires wider delivery-semantics design and is a deferred candidate.

## Implementation

- Implementation subagent: `b0047_implementation`
- Complexity: simple
- Plan summary: clamp explicit flush intervals to 1–60,000 ms in `config_mapping.rs` and add colocated zero/ceiling regression coverage.
- Actual change: introduced a 1 ms minimum for explicit values while preserving the absent 5-second default and 60-second ceiling.
- Behavior intentionally preserved: all positive in-range values and authentication, TLS, egress, health, queue, retry, shutdown, schema, protocol, logging, and metrics behavior.
- Tests added or updated: colocated tests cover zero, absent/default, and oversized values; no integration-test edit or gap is required.

## Independent review

- Reviewer: `b0047_independent_review`
- Verdict: LGTM after correction; the zero-period panic path, clamp semantics, regression tests, unchanged consumers, and five no-change files were independently confirmed.
- Must Fix resolution: the first targeted compile exposed an invalid test-only `Default` struct update; `b0047_test_diagnostic` attributed it to the batch, the implementer changed it to an explicit `EsBulk` literal, 15 focused tests passed, and the reviewer approved the latest diff.
- Deferred observations: `D0007` records the broader item-level transient retry contract.
- Adjudication: none.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | 2,681 existing paths matched the manifest |
| Independent static review | passed | Full module and diff review confirmed the local clamp, tests, preserved behavior, and deferred retry boundary |
| Targeted unit tests | passed | 15 Elasticsearch config-mapping tests passed after the diagnostic correction |
| Formatting | passed | `cargo fmt --all` and final `cargo fmt --all -- --check` |
| Workspace compile | passed | `cargo check --workspace --all-targets` |
| Workspace lint | passed | `cargo clippy --workspace --all-targets`; existing warnings remain non-fatal |
| Repository guards | passed | Agent docs, SSA force, metrics inventory, Pingora source policy, and Gateway no-tracing |
| Frozen integration paths after verification | passed | Existing `edgion-tests/**` files remained unchanged |

## Handoff

- Ledger/changelog updated: yes
- Commit: `03001c49`
