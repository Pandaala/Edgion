# B0027 — LB Consistent Hash

## Scope

- Baseline: `29ec67ff9abe66be71f80a06156816a413893ff5`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- File: `edgion-gateway/src/lb/selection/consistent_hash.rs`
- Approved knowledge support: `skills/04-review/lb/regression-guards/rr-consistent-hash-per-backend-weight-cap.md`
- Forbidden files: every other source or knowledge file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the source and knowledge files above plus B0026/B0027 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0027 analysis turn.
- Decision: `change`, simple. The locked `pingora-ketama` casts every bucket node index to `u16`, while ConsistentHash previously admitted an unbounded Inet backend count. A 65,536th prepared node could wrap to index zero and assign its virtual nodes to the wrong address.
- Boundary decision: match the accepted RoundRobin count contract of 65,535 backends. Filter and cap a single ordered Inet input before deriving both Ketama buckets and address indices.
- Preserved semantics: original input indices, complete backend vector, weight clamp, deterministic order and hashing, health filtering, distinct-address termination, empty-key RR fallback, and generation pinning.
- Observability decision: add no duplicate event or metric because the eagerly built RR selector already reports the identical oversized generation once.
- Freshness revalidation: valid at `29ec67ff9`; B0026 changed only HTTP retry preparse and campaign records.

## Implementation

- Added the RR-aligned 65,535-Inet-backend cap and one preparation function that retains original indices and produces both ring buckets and `addr_to_idx` from the same bounded sequence.
- Added a preparation-boundary regression with one skipped Unix backend and 65,536 unique Inet backends, avoiding construction of roughly ten million Ketama points while proving the cap, map/ring coherence, original index retention, and overflow exclusion.
- Corrected stale review knowledge that had described non-Inet filtering as a ConsistentHash count bound.
- Removed three batch-introduced test Clippy warnings with an independently reviewed mechanical `match` to `let-else` correction.

## Independent review

- Reviewer: `b0001_review`, independent from the B0027 analyst and implementer.
- Verdict: approved with no findings after checking the locked dependency cast, conservative off-by-one contract, common prepared-input ownership, original/full-vector indexing, fallback and health semantics, arithmetic bounds, regression cost, observability restraint, knowledge accuracy, and correction diff.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | ConsistentHash 14, complete selection 33, and compiled-selector 8 tests passed. |
| Independent review checks | passed | Gateway check/Clippy, formatting, agent docs, no-tracing, English-only, diff, exact scope, and frozen checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing Kafka/frozen-test lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: `be90cfc13133c6b8cd9fd00dca298936e38b5280`.
