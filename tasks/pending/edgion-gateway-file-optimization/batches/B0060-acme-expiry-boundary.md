# B0060 — ACME Challenge Expiry Boundary

## Scope
- Baseline: `564e77babda5a1fe69c3b1f48b38a74ac04da025`
- Pipeline state: reviewed and fully verified; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/services/acme/{challenge_store.rs,conf_handler_impl.rs,mod.rs}`; `edgion-gateway/src/services/mod.rs`
- Exact implementation file: `edgion-gateway/src/services/acme/challenge_store.rs`
- Allowed tests: colocated tests in the implementation file only
- Forbidden files: every existing `edgion-tests/**` path, controller/resources/request-filter/runtime code, metrics, docs, other service modules, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: the implementation file; campaign ledger/changelog/main record; B0059 and B0060 batch records; reviewer-approved gatewayRef and integration-coverage task records plus campaign deferred backlinks; legacy follow-up index

## Context and value gate
- Analysis subagent: `b0056_implementation`, promoted after revalidation against `564e77ba`
- Baseline: all four target blobs are unchanged since the queued report; challenge-store tests pass 10/10.
- Concrete opportunity: `lookup` currently serves a challenge when `now == expire_at`, although `expireAt` is an absolute expiration timestamp and the Gateway must ignore expired challenges.
- Decision: make the expiry deadline exclusive with a private controlled-time lookup seam and `now >= expire_at`; add deterministic immediately-before and exact-boundary coverage without sleeps, timers, network, cleanup scans, or integration changes.
- Expected no-change files: the conf handler retains true incremental atomic publication and bounded metadata-only config logging; both module files retain their minimal live facade/export boundaries.
- Preserved behavior: host normalization and binding, empty-host handling, token lookup, `Bytes` clone semantics, ArcSwap hot-path reads, full/partial/delete publication, fixed metric labels, and absence of token/key-authorization logging.

## Deferred candidate
- The raw `gatewayRef` reaches the Gateway inside the serialized EdgionAcme, but the Controller does not validate or resolve it into a serving attachment or register Gateway dependency requeue; the Gateway store and pre-route interception do not enforce target ownership, so every synced challenge is globally eligible by host/token.
- The correction is a complex Controller/resources/Gateway listener-ownership design and must not hitchhike on the local expiry predicate.
- Existing integration tests remain frozen. The wider task genuinely needs a separate new integration-coverage issue for positive selected-Gateway service, negative non-selected-Gateway fallthrough, target hot update, and challenge removal; no existing integration file will be edited in B0060.

## Implementation
- Implementer: `b0055_runtime_analysis`
- Complexity: simple, one predicate plus deterministic local seam/tests
- Plan: public lookup samples epoch seconds once, delegates to a private controlled-time lookup, and rejects equality with `>=`; add exact pre-boundary/equality assertions and preserve all other matching behavior.
- Actual change: public lookup samples epoch seconds once and delegates to a private controlled-time seam; the expiry predicate now rejects equality with `now >= expire_at`; deterministic coverage proves the immediately-before hit and exact-boundary miss.

## Independent review
- Reviewer: `b0056_final_review`
- Code verdict: Approved with no code findings
- Record correction: clarified that raw gatewayRef already traverses conf-sync; the missing contract is validated/resolved serving ownership and enforcement.
- Deferred adjudication: D0026 records the nonduplicate technical bug and the later multi-Gateway HTTP-01 isolation scenario. D0027 is closed as not a standalone defect; see `skills/04-review/testing/intentional-design/acme-gatewayref-isolation-coverage-not-standalone.md`. Existing integration tests remain frozen.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | user-authorized concurrent baseline unchanged |
| Baseline challenge-store tests | passed | 10 tests |
| Implementation and focused tests | passed | 11 challenge-store and 11 ACME module tests; Gateway all-target compile passed |
| Independent code review | passed | 11 focused tests, formatting, diff, and frozen checks passed |
| Final focused tests | passed | 11 ACME service tests after formatting |
| Full formatting | passed | `cargo fmt --all` |
| Workspace compile | passed | `cargo check --workspace --all-targets` |
| Workspace Clippy | passed | exit 0 with existing repository warnings |
| Agent documentation | passed | 1,114 files, 36 skill entries, and 711 source-path scopes checked |
| SSA force guard | passed | 1,432 files checked; one existing allowlisted site |
| Metrics inventory | passed | 103 metrics in sync |
| Pingora source policy | passed | 16 fork packages share `edgion_v3` and one locked revision |
| Gateway no-tracing guard | passed | data-plane boundary and cold-function registry clean |
| Final frozen integration paths | passed | user-authorized concurrent baseline unchanged by B0060 |

## Handoff
- Ledger/changelog updated: yes; one changed and three examined-no-change
- Commit: `e15676a9`
