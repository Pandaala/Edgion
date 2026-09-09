# B0204 — TLS Listener Tables

## Scope

- Pipeline state: verified; awaiting local commit.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/tls/gateway_tls_routes.rs`.
- Baseline: `9cb9bf86`.
- Freshness confirmed at `b29c0701`, including B0203 schema ownership; target/manager/proxies/knowledge unchanged, fresh target tests 4/4.
- Analyst: `b0204_tls_listener_routes_analysis`.
- Investigate listener-keyed table ownership, matching, publication, clear and
  retained handles, hot reload and stats against current shared boundaries.
- Existing integration files and user-owned paths remain protected; implementation
  follows the approved three-file plan below.

## Analysis Outcome

- Decision: change; complex/plan-first for winner-only published storage. Target blob `b4d9ec02b57d20175536baaf0364b5e024d149ae`; target baseline 4/4 passed.
- Borrow hostname slices instead of deep-copying Vec/String inputs before lowercasing. Retain temporary candidate vectors and collision ordering/warnings during construction, then publish only the selected Arc per hostname/catch-all; all runtime consumers use first(), and stats do not enumerate candidate vectors.
- Exact proposed scope: target; `skills/04-review/tls/regression-guards/cross-gateway-sni-collision-deterministic.md` for this matcher's storage only; `skills/01-architecture/02-gateway/03-routes/04-tls-route.md` for table/from_units/effective-hostname details.
- Preserve exact > longest wildcard > catch-all, case/multi-depth wildcard behavior, timestamp/name precedence, raw/effective hostname semantics, unchanged unit sharing, collision warnings, and old snapshot validity. Test runner-up promotion after winner deletion and Weak evidence that published tables no longer retain losers.
- Existing tests stay; run all TLS route tests. Independent plan approval is required before code changes.
- High-severity attachment-scoping defect is separately tracked in D0114. Analyst confirms safe continuation with equivalent matching/storage optimization, without claiming to fix listener isolation.

## Complex Implementation Plan

- Implementer: `b0204_implementation`; plan-only, no edits.
- Plan reviewer: `b0204_plan_review`; Approved, no Must Fix, fresh at external HEAD `264847df`. Preserve equal-priority input order and warnings. Winner-only releases published table ownership, not configured losers still owned by the canonical route cache.
1. Borrow effective/raw hostname slices, retaining resolved-listener branch and empty-intersection behavior; allocate only normalized bucket keys.
2. Preserve temporary candidate vectors, stable precedence sorting and existing collision warning order, then consume the first winner into `HashHost<Arc<TlsRouteUnit>>` and optional catch-all Arc.
3. Preserve public owned-Arc match API, HashHost Gateway wildcard semantics and unit/selector sharing; use singular private catch-all state.
4. Retain all four tests and add precedence/case/depth, effective/raw/empty, multi-SNI identity, Weak loser-release and new-rebuild runner-up/old-snapshot lifetime coverage.
5. Update only the two owning documents; distinguish current route-global projection from D0114's unfixed isolation requirement.
6. Run focused format, target/all-TLS tests and diff checks, then independent code review. Any equivalence/scope failure returns for plan review; rollback only owned changes through explicit patches.

## External Reconciliation

- `b0204_external_sync_diagnosis` confirmed external `264847df` changes only Redis-related code/knowledge and one new test; no TLS plan input changed. The added unification guidance remains Redis-specific.
- All 3,119 old frozen entries passed unchanged. The new committed Redis script lifecycle test's bytes/mode matched HEAD and was added by exact path; freeze now contains 3,120 files. No integration worktree/index edits occurred.
- Three externally changed Gateway Redis rows were reopened pending with prior B0061/B0097 history retained; no Gateway files were added or removed. This reconciliation is not a Redis correctness review.
- External Redis knowledge linked a research task absent from the worktree, tracked files and local history. Independent diagnosis rejected invented restoration. Plan reviewer approved one additional support paragraph in `skills/04-review/rate-limit/regression-guards/redis-script-cache-lifecycle.md`: cite the canonical executor and committed lifecycle test, retain the missing research attribution as unverifiable, and preserve historical test records. The original TLS plan is unchanged; final reviewer checks this exact support edit.

## Implementation and Final Review

- External `256733d7` only updated knowledge and closed D0075; independent diagnosis verified current blocking-stage ownership and reconciled historical links without restoring deleted tasks or claiming fresh Wasm tests. Source and integration baselines are unchanged.
- Implementer completed the approved plan and support amendment in exactly four technical files. Original four tests retained; five matrix/lifetime/rebuild tests added.
- Target tests 9/9, all TLS route tests 48/48; format, diff and agent-doc checks passed.
- Final reviewer: `b0204_final_review`; LGTM, no Must Fix or Should Fix. Reviewed target blob `ac641911487b1b445977bac52cf130b0445933c2` at external HEAD `256733d7`; independently checked both consumers, HashHost, plan equivalence and all support documents. Reviewer format, diff and documentation checks passed; no reviewer test completion is claimed.

## Verification

- Workspace format, agent-doc (1,262 files), SSA, metrics, Pingora, Gateway tracing, English-only, diff and 3,120-file freeze checks passed.
- Locked workspace check/Clippy and Gateway binary build passed. Broader route tests passed 591/591; coordinator reran TLS tests at `256733d7`, passing 48/48. Clippy retains existing warnings, with no new batch finding in independent review.

## Handoff

- Commit: pending local commit; review and verification gates passed. Never push.
