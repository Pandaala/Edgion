# B0051 — LinkSys OTLP Provider

## Scope
- Baseline: `3b470e32a88ea069658cc22f50fed049fb2b38c8`
- Pipeline state: verified
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/link_sys/providers/otlp/{log_sender.rs,mod.rs}`
- Allowed support files: colocated unit tests in the two target files only; no cross-crate support is authorized
- Forbidden files: every existing `edgion-tests/**` path, logging producers, other providers, runtime/template files, shared schemas, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact commit allow-list: the two target files; `03-subtasks.md`; `07-changelog.md`; `edgion-gateway-file-optimization.md`; the B0050 and B0051 batch records; one verified OTLP integration deferred record and `deferred/INDEX.md`

## Context and value gate
- Analysis subagent: `b0051_candidate_analysis`; report revalidated unchanged against promotion HEAD.
- Coverage summary: reviewed both OTLP provider files, access-log metadata construction and JSON masking, sensitive-mode initialization, DataSender batching/health/shutdown precedents, outbound probe policy, schemas, and consumers.
- Concrete opportunity: `RequestInfo.normalized_current_path` is canonically masked in the serialized access-log body, but the OTLP sender copies the raw `LogMeta.path` into the parallel `url.path` attribute, bypassing the configured sensitive-data policy.
- Expected benefit: apply the same canonical sensitive policy to every path representation in one OTLP record without changing its schema or delivery behavior.
- Regression surface: the value of the existing `url.path` OTLP attribute under `off`, `replace`, and `wrap` sensitive modes.
- Decision: change `log_sender.rs`; examine `mod.rs` with no expected change. A stale OTLP integration scenario should be recorded separately rather than edited.

## Implementation
- Implementation subagent: `b0051_implementation`
- Complexity: simple; one serialization surface and deterministic colocated tests.
- Plan summary: use the canonical sensitive helper at the OTLP attribute boundary and prove all three configured modes without changing the JSON body or sender lifecycle.
- Actual change: routed `LogMeta.path` through the canonical `apply_sensitive` helper before adding the existing OTLP `url.path` attribute.
- Behavior intentionally preserved: the serialized access-log body, attribute name and presence, method, status, severity, timestamps, batching, health probe, endpoint parsing, and shutdown behavior.
- Tests added or updated: three colocated subprocess-isolated regressions cover `off`, `replace`, and `wrap`; existing integration tests remain frozen and their stale coverage is tracked by D0012.

## Independent review
- Reviewer: `b0051_independent_review`
- Verdict: LGTM after correcting the initial module-local global-mode lock.
- Must Fix resolution: mode-specific tests now run themselves in exact isolated subprocesses, so the parent lib-test process never mutates global `SENSITIVE_MODE` and cannot race other modules.
- Deferred observations: [`D0012`](../deferred/D0012-otlp-integration-masking-health-refresh.md) records the frozen OTLP integration scenario's weak path assertion and stale always-healthy limitation.
- Adjudication: reviewer confirmed D0012 is real, correctly scoped, and non-duplicative; D0011 continues to own the shared sender shutdown contract.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | 2,681 existing paths matched the manifest |
| Focused unit tests | passed | 10 OTLP sender tests passed, including three isolated sensitive-mode cases |
| Independent static review | passed | Canonical masking, single application per output surface, schema/lifecycle preservation, and subprocess isolation were approved |
| Format | passed | `cargo fmt --all`; preflight `cargo fmt --all -- --check` passed |
| Workspace compile and lint | passed | `cargo check --workspace --all-targets` and `cargo clippy --workspace --all-targets` exited zero |
| Repository guards | passed | Agent docs, SSA force, metrics inventory, Pingora source policy, and Gateway no-tracing checks passed |
| Frozen integration paths after verification | passed | 2,681 existing paths still matched the manifest; D0012 records the future integration-only update |

## Handoff
- Ledger/changelog updated: yes
- Commit: pending automatic commit
