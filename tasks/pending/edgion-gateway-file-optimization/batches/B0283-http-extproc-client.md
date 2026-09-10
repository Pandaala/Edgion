# B0283 — HTTP ExtProc Client

- Pipeline state: terminal; changed, committed as `6bf48dcf`.
- Active anchor: `edgion-gateway`; refreshed baseline `bd84aa15`.
- Target: `edgion-gateway/src/plugins/http/ext_proc/client.rs`.
- Analyst: `b0283_extproc_client_analysis`; change after full transport/lifecycle/shared-mechanism and coverage analysis.
- Analysis resumed after a provider quota interruption and was refreshed after B0282 committed. Target blob `807f1a7ff143420c41c990813df33230c792da6e` was unchanged at promotion.
- All integrations and user handoff/WAF paths remain protected; only the frozen technical scope is authorized.
- Evidence: controller capability guard is advisory, shared validation lacks the guard, and gateway construction currently ignores these constraints. Constructor errors already reach sealed fail-closed resource activation. Preserve CA-only and empty/absent SAN behavior, inheritance and local block replacement semantics.
- Security/transport-boundary change used plan-first. Nonnetwork constructor/explicit-default and sealed-activation tests only; no global TLS mutation or live-server tests.
- Separate findings: D0150 dynamic unsupported-policy recovery integration gap; D0151 live gRPC unit-tier isolation. D0148/D0149 remain separately owned.
- B0282 committed bd84aa15. Original analyst refreshed current rules/knowledge/dependencies and reaffirmed change with unchanged client blob. D0149 now owns obsolete phase knowledge; do not duplicate it.
- Exact technical allow-list: client.rs, `skills/04-review/ext-proc/SKILL.md`, and new `skills/04-review/ext-proc/regression-guards/unsupported-outbound-tls-identity-fails-closed.md`. No schema or cross-crate source edits.
- Implementer: `b0283_implementation`. In-file pure resolved-policy and in-memory compile/sealed-filter tests avoid store publication, network and global-default/provider mutation. Seven existing tests selected exactly, never broad live_stream/global-mutation filters.
- Bookkeeping scope: ledger/main/changelog, this record, B0282 SHA backfill, B0284 candidate record, and independent deferred task/backlink/index. Existing integrations and handoff/WAF paths remain protected.
- Plan reviewer `b0283_plan_review`: Approved at bd84aa15, no Must Fix. Canonical resolution and compiler seal chain independently checked; builder success is not handshake proof and pure tests do not prove live reload/no-dial/recovery.
- Add the B0285 read-only candidate record to bookkeeping scope; no next-batch implementation or review before this batch commits.
- First test compilation failed E0609 in the new certificate fixture (`key_pair` does not exist in locked rcgen 0.14.8). Implementer corrected to `signing_key` and reran before receiving the coordinator hold: a recorded procedural deviation, not an omitted failure.
- Fresh `b0283_fixture_diagnostic` independently confirmed batch attribution and the one-field correction against locked source/current callers. Correction remains within the approved fixture scope; rerun passed. Remaining verification resumed after diagnostic routing; no production or dependency change needed.
- Implementation complete in the three approved files; four new and seven existing exact nonnetwork tests passed, scoped formatting/whitespace checks passed. Fresh `b0283_final_review` assigned original rules, full files/call chains and knowledge verification.

## Approved plan

1. Add a private effective-TLS capability guard immediately after canonical resolution, before hashing/cache/channel construction; reject present hostname before nonempty SAN with static value-free errors.
2. Add constructor rejection tests for hostname, empty hostname, SAN, both and verify-false plus SAN; no lazy channel is reached.
3. Test explicit-default inheritance and whole-block replacement without global mutation; cover supported CA, mTLS and skip-verify construction with explicit providers.
4. Compile in-memory invalid plugin resources under both failure policies; assert sealed activation and execute the existing generic rejection filter without store publication or network.
5. Add the owning regression guard and index link, distinguishing unsupported configuration rejection from runtime failure policy and future TLS support.
6. Run exact new and seven named nonnetwork existing tests, then independent final review and workspace gates. Amend/re-review any scope or semantic deviation; only recover owned hunks.

## Verification

- At bd84aa15 plus implementation: four new and seven existing exact tests passed after the independently diagnosed fixture correction.
- Workspace check passed (11.29s); Clippy passed (15.84s, existing warnings); formatting/format check, agent docs, SSA, metrics, Pingora source, Gateway no-tracing, English, frozen paths and whitespace checks passed.
- Final reviewer `b0283_final_review`: LGTM, no Must Fix or plan-semantic disagreement. Full code, original knowledge and constructor-to-sealed-filter chain independently verified.
- No integration execution/edit, live network test, global provider/default mutation or store publication. Ledger/changelog updated; commit `6bf48dcf`, never push.
