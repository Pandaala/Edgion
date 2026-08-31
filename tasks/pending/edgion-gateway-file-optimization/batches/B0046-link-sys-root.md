# B0046 — LinkSys Root

## Scope

- Baseline: `3d788d49e2482a47ab0ed23f576ee665521e5c73`
- Pipeline state: terminal
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/link_sys/mod.rs`
- Allowed support files: colocated unit tests in the target file only; no cross-crate support is authorized before analysis proves necessity
- Forbidden files: every existing `edgion-tests/**` path, unrelated LinkSys provider/runtime/template files, unsupported Gateway rustls code, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact commit allow-list: the target file; `03-subtasks.md`; `07-changelog.md`; `edgion-gateway-file-optimization.md`; `deferred/INDEX.md`; any analyst-verified new deferred record; the B0045 and B0046 batch records

## Context and value gate

- Analysis subagent: `b0046_analysis`
- Coverage summary: reviewed the full root, provider/runtime/template modules, shared LinkSys core, conf-sync lifecycle, active consumers, configuration variants, existing tests, and review precedent.
- Key responsibility/call chain: this cold facade exposes shared LinkSys primitives and Gateway-owned provider/runtime APIs used by conf-sync, admin, access logging, AI, DSL, health, and plugin consumers.
- Concrete opportunity and decisive evidence: the opening architecture documentation names nonexistent ClickHouse runtime support, places shared-core Etcd under the Gateway, omits live Elasticsearch/Kafka ownership, and conflates LinkSys CRD providers with LocalFile/OTLP logging sinks.
- Expected benefit: keep contributor-facing ownership and provider boundaries aligned with the enforced crate architecture without changing runtime behavior.
- Regression surface: comment-only; every module declaration and public re-export must remain byte-for-byte unchanged.
- Decision: change; no deferred finding.

## Implementation

- Implementation subagent: `b0046_implementation`
- Complexity: simple
- Plan summary: replace only the opening module documentation with the current Gateway/shared-core/provider/sink ownership model.
- Actual change: corrected the provider inventory and crate ownership, removed the unsupported ClickHouse claim, and separated non-runtime SystemConfig variants from logging sinks.
- Behavior intentionally preserved: every module declaration, public re-export, executable line, API, schema, protocol, log, metric, and failure-policy path.
- Tests added or updated: none; the approved change is documentation-only and creates no integration-test gap.

## Independent review

- Reviewer: `b0046_independent_review`
- Verdict: LGTM; every documentation claim matches the current schema, provider ownership, runtime dispatch, typed accessors, resolver path, and logging-sink boundary.
- Must Fix resolution: none required.
- Deferred observations: none.
- Adjudication: none.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | 2,681 existing paths matched the manifest |
| Independent static review | passed | Full-file and full-diff review confirmed a comment-only architecture correction with no runtime or API change |
| Formatting | passed | `cargo fmt --all` and final `cargo fmt --all -- --check` |
| Workspace compile | passed | `cargo check --workspace --all-targets` |
| Workspace lint | passed | `cargo clippy --workspace --all-targets`; existing warnings remain non-fatal |
| Repository guards | passed | Agent docs, SSA force, metrics inventory, Pingora source policy, and Gateway no-tracing |
| Frozen integration paths after verification | passed | Existing `edgion-tests/**` files remained unchanged |

## Handoff

- Ledger/changelog updated: yes
- Commit: pending automatic commit
