# B0055 — Borrowed Access Log Metadata

## Scope
- Baseline: `99cc5f7a11e98fbec603dbe2b758d9c447b9cf85`
- Pipeline state: reviewed; pending full verification and commit
- Active anchor crate: `edgion-gateway`
- Owned target: `edgion-gateway/src/link_sys/runtime/data_sender.rs`
- Exact support files: `edgion-gateway/src/observe/access_log/logger.rs`, `edgion-gateway/src/routes/http/proxy_http/pg_logging.rs`, and `edgion-gateway/src/link_sys/providers/otlp/log_sender.rs`
- Allowed tests: colocated unit tests in the four owned files only
- Forbidden files: every existing `edgion-tests/**` path, output schemas, other senders/providers, shared crates, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact commit allow-list: the four owned code files; `03-subtasks.md`; `07-changelog.md`; `edgion-gateway-file-optimization.md`; B0054 and B0055 batch records; D0023; and `deferred/INDEX.md`

## Context and value gate
- Analysis subagent: `b0055_runtime_analysis`
- Coverage summary: traced the complete `ProxyHttp::logging` to `pg_logging` to `AccessLogger` to `DataSender` chain and every LocalFile, Elasticsearch, Kafka, and OTLP implementation and test.
- Concrete opportunity: `LogMeta` owns method and path strings, forcing two heap allocations for every HTTP terminal log call even when access logging is disabled or the selected sender ignores metadata; only OTLP consumes the fields.
- Expected benefit: make metadata a call-scoped borrowed view and move ownership to the actual OTLP attribute construction point.
- Regression surface: async trait-object lifetimes, first-healthy selection, canonical access-log finalization, OTLP field/masking parity, severity/status/timestamp behavior, and disabled logging.
- Decision: change the target and three exact support files. The remaining LinkSys runtime targets are split into a later health-clock batch or examined-no-change outcomes.

## Implementation
- Implementation subagent: `b0055_implementation`
- Complexity: complex; plan-first review is required before editing
- Plan summary: make `LogMeta<'a>` borrow method/path through object-safe async sender dispatch; pass the live rewritten method and canonical normalized path directly from terminal logging; keep first-healthy selection unchanged; own method/path only while OTLP constructs attributes and preserve canonical path masking; prove local-string and trait-object borrowing, including pointer identity.
- Actual change: `LogMeta<'a>` now borrows method/path through async trait-object dispatch; terminal logging passes the live rewritten method and canonical normalized path without allocation; OTLP alone materializes values at attribute construction and keeps the existing sensitive-path helper.
- Behavior intentionally preserved: access-log JSON/finalization and integration capture, first-healthy/no-fallthrough/drop behavior, disabled logging, mirror cleanup, status/severity/timestamps, OTLP attribute names/types, and masking.
- Tests added or updated: local-string and `Box<dyn DataSender<String>>` tests assert value and pointer identity; existing AccessLogger and OTLP sender behavior tests remain intact; no integration-test file changed or is needed.

## Independent review
- Plan reviewer: `b0055_plan_review`
- Plan verdict: Approved
- Final code reviewer: `b0055_final_review`
- Final verdict: Approved with no code findings
- Deferred adjudication: D0023 records the independently confirmed OTLP dial-time egress and endpoint-identity risk; system DNS and SDK-managed TLS remain investigation questions rather than asserted violations.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | user-authorized concurrent baseline unchanged |
| DataSender tests | passed | 2 tests |
| AccessLogger tests | passed | 6 tests |
| OTLP sender tests | passed | 10 tests |
| Gateway all-target compile | passed | borrowed async trait-object path compiles across every sender and caller |
| Independent code review | passed | allocation ownership, finalization, delivery policy, schema, and masking verified |
| Gateway no-tracing guard | passed | no data-plane tracing violation |
| Diff whitespace check | passed | no error |
| Full formatting | passed | `cargo fmt --all` |
| Workspace compile | passed | `cargo check --workspace --all-targets` |
| Workspace Clippy | passed | exit 0 with existing repository warnings |
| Agent documentation | passed | 1,114 files, 36 skill entries, and 711 source-path scopes checked |
| SSA force guard | passed | 1,432 files checked; one existing allowlisted site |
| Metrics inventory | passed | 103 metrics in sync |
| Pingora source policy | passed | 16 fork packages share `edgion_v3` and one locked revision |
| Final focused tests | passed | 2 DataSender, 6 AccessLogger, and 10 OTLP tests after formatting |
| Final frozen integration paths | passed | user-authorized concurrent baseline unchanged by B0055 |

## Handoff
- Ledger/changelog updated: yes; four owned rows close as changed
- Commit: `7cca648b`
