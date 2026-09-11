# B0048 — LinkSys Kafka Provider

## Scope

- Baseline: `03001c4914abf24b4f4342a4cc4193ac350b82fd`
- Pipeline state: terminal
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/link_sys/providers/kafka/{client.rs,log_sender.rs,mod.rs}`
- Allowed support files: colocated unit tests in the three target files only; no cross-crate support is authorized before analysis proves necessity
- Forbidden files: every existing `edgion-tests/**` path, unrelated LinkSys providers/runtime/template files, shared schemas or common mechanisms without exact necessity evidence, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact commit allow-list: the three target files; `03-subtasks.md`; `07-changelog.md`; `edgion-gateway-file-optimization.md`; legacy follow-up index; any analyst-verified new deferred record; the B0047 and B0048 batch records

## Context and value gate

- Analysis subagent: `b0048_analysis`
- Coverage summary: reviewed all three provider files, resource and Secret resolution, canonical outbound TLS construction, runtime publication, per-topic bounded queues, producer lifecycle, health/probe behavior, integration coverage, and review precedent.
- Key responsibility/call chain: Kafka output obtains a published client, performs non-blocking per-topic queue admission, and background producers batch and send through rskafka while publishing health.
- Concrete opportunity and decisive evidence: accepted `channelSize: 0` reaches `tokio::mpsc::channel(0)` and panics on first send; accepted `lingerMs >= 30_000` cannot complete before the fixed 30-second produce deadline and causes deterministic timeout/respawn churn.
- Expected benefit: fail fast during client construction instead of panicking or entering a permanent delivery-failure loop.
- Regression surface: invalid boundary values only; defaults, valid values, non-blocking admission, TLS/Secret, health, payload/topic, and existing integration behavior remain unchanged.
- Decision: change; producer-generation shutdown, aggregate topic/channel memory bounds, and rskafka broker egress/DNS policy are deferred candidates.

## Implementation

- Implementation subagent: `b0048_implementation`
- Complexity: simple
- Plan summary: validate resolved channel size and linger duration in `client.rs`, then add two colocated unit regressions.
- Actual change: reject zero queue capacity and linger durations at or beyond the fixed produce timeout before constructing or publishing a client.
- Behavior intentionally preserved: defaults, all valid values, non-blocking admission, fire-and-forget delivery, TLS/auth, health/probe, topic/payload, logging, and metrics.
- Tests added or updated: two colocated constructor regressions; no integration-test edit or gap is required.

## Independent review

- Reviewer: `b0048_independent_review`
- Verdict: LGTM; validation order, units, exact timeout boundary, error distinction, preserved publication behavior, and both no-change files were independently confirmed.
- Must Fix resolution: none required.
- Deferred observations: `D0008`, `D0009`, and `D0010`.
- Adjudication: none.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | 2,681 existing paths matched the manifest |
| Independent static review | passed | Full module and diff review confirmed both fail-fast guards, tests, preserved behavior, and three deferred boundaries |
| Targeted unit tests | passed | 12 Kafka client tests passed |
| Formatting | passed | `cargo fmt --all` and final `cargo fmt --all -- --check` |
| Workspace compile | passed | `cargo check --workspace --all-targets` |
| Workspace lint | passed | `cargo clippy --workspace --all-targets`; existing warnings remain non-fatal |
| Repository guards | passed | Agent docs, SSA force, metrics inventory, Pingora source policy, and Gateway no-tracing |
| Frozen integration paths after verification | passed | Existing `edgion-tests/**` files remained unchanged |

## Handoff

- Ledger/changelog updated: yes
- Commit: `a7b19937`
