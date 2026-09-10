# B0285 — HTTP ExtProc Mutation

- Pipeline state: terminal; changed, independently reviewed and verified, pending local commit.
- Active anchor: `edgion-gateway`; baseline `989d2a27`.
- Target: `edgion-gateway/src/plugins/http/ext_proc/mutation.rs`.
- Analyst: `b0285_extproc_mutation_analysis`; full mutation/consumer, boundary and coverage/value analysis assigned.
- B0283/B0284 committed. Since the analyst's 6bf48dcf refresh, only coordinator records changed; source/knowledge evidence remains current. B0285 owns the serial lane.
- Integrations and user handoff/WAF files remain protected; no technical mutation authorized.
- Target blob `850ac44c51637a5b2e0e6a654a34300743887205`; complete collector/mutator and consumers reviewed. get_path omits the query in ExtProc :path; canonical get_uri uses current normalized path and raw query, preserving prior rewrites.
- Proposed scope: mutation.rs plus only the plugin.rs expect_empty_request_headers mock expectation adaptation. Cover query-free and duplicate headers, duplicate query keys, percent-encoded separators, plus and empty query with contrasting current/frozen views. Run only the deterministic mutation test module; no live plugin tests.
- Separate candidate finding: text-only collection, lossy raw-value decoding and text-based existence checks lose opaque header fidelity. Requires its own end-to-end contract task, not part of the query fix. D0149 already covers stale phase knowledge.
- Analyst corrected its initial simple label: protocol-boundary behavior requires complex plan-first regardless the small diff. Revalidate at promotion after B0283/B0284 commits.
- Original analyst refreshed at 6bf48dcf: target blob unchanged; plugin support blob `9390e125a91282119939fba39bd8caca59222555`. New client TLS rejection does not alter header collection or the query omission. Scope unchanged; revalidate once more after B0284 records commit.
- `b0285_implementation` assigned plan only. Technical scope is mutation.rs plus only the plugin.rs mock expectation adaptation. No opaque-header or production plugin edits authorized.
- Bookkeeping scope: this record, ledger/main/changelog, B0284 SHA backfill, B0286 candidate refresh and independently verified deferred task/backlink/index. No integration changes.
- Implementer returned the following plan; independent `b0285_plan_review` assigned. Canonical compose_uri drops an empty query delimiter, so the test must preserve /api? to /api rather than introduce a new URI rule.
- Add B0287 read-only candidate bookkeeping to this batch's record scope; no next-batch implementation or review before commit.
- Plan reviewer `b0285_plan_review`: Approved at 989d2a27, no blocker. Literal current-URI assertions and distinct entry/current real-context setup verified; D0152 independently supported. Original implementer authorized within the two-file scope.
- Implementation completed without deviation: canonical query projection, five-case real-context regression, and one plugin helper expectation only. Offline mutation tests passed 12/12; scoped format/whitespace checks passed. Fresh `b0285_final_review` assigned full code/rules/knowledge and D0152 verification.

## Approved plan

1. Revalidate baseline and exact mutation.rs/plugin.rs allow-list; protect unrelated work.
2. Project :path through canonical get_uri and clarify current normalized path/raw query ownership; no ad-hoc composition.
3. Retain original query-free/duplicate-header assertions; test ordinary/duplicate/encoded/plus/empty queries and distinct entry/current views using real context composition behind the mock.
4. Adapt only the plugin test helper's accessor expectation; preserve all mutation/framing/raw-header behavior and live test structure.
5. Run the deterministic mutation test module and formatting, then independent review and project gates; no integration or live-server tests.
6. Check original hard rules and scope; amend/re-review any material deviation and recover only campaign-owned hunks.

## Verification and handoff

- Final reviewer `b0285_final_review`: LGTM at 989d2a27 plus diff, no Must Fix or plan deviation. All three production consumers and real-context assertions independently verified; D0152 reconfirmed separately.
- Mutation tests passed 12/12; workspace check passed (11.07s), Clippy passed (16.91s, existing warnings). Formatting/format check, docs, SSA, metrics, Pingora source, Gateway no-tracing, English, frozen paths and whitespace checks passed.
- Exact technical scope remained mutation.rs plus one plugin test-helper expectation. No integration execution/edit or unrelated fixes. Ledger/changelog updated; pending local commit, never push.
