# B0290 — HTTP Forward Auth Plugin

- Pipeline state: changed; committed after independent plan/final LGTM and all checks.
- Active anchor: `edgion-gateway`; promoted baseline `99069928` (target blob unchanged from analysis).
- Target: `edgion-gateway/src/plugins/http/forward_auth/plugin.rs`.
- Analyst: `b0290_forward_auth_analysis`; full current target, external changes, call chains, original rules/knowledge and deterministic test/value coverage assigned. Prior B0077 evidence is historical; external 9b787978..b297af6e reopened the file.
- B0288 owns the serial lane; B0289 precedes this candidate. Refresh before promotion. Preserve external task resolutions and deduplicate existing findings.
- Explicit gates include canonical outbound client/TLS/egress/key extraction, body limits/streaming, failure policy, binary/access-log ownership, redaction/restraint/unification and English-only content. No edits, builds/tests/network, integration additions/edits or protected user-path changes.
- Full target/current consumers examined; blob `0ba941cd9c8082708ab560dd7abb861791c4c709`. `send_error_response` selects only HeaderMap::get and inserts one value, losing subsequent WWW-Authenticate/Set-Cookie values. B0077 preserved observed behavior, not an accepted exception to current repeated-response-header rule P.
- Proposed exact technical scope: target; new `skills/04-review/forward-auth/regression-guards/deny-response-repeated-headers-preserved.md`; owning forward-auth knowledge index. Iterate retained values using first insert/subsequent append without an owned value collection; duplicate/case-equivalent configured names must not multiply output. Preserve current text filtering, success-side identity behavior, body transfer, framing and write/shutdown order. Target's selective request example may also add the omitted forwardAll:false setting.
- Proposed mock assertions: separate ordered challenges and cookies, duplicate configured names, status/content-type/body ownership/EOS/shutdown/unselected-header behavior. Existing safe module filter selects seven mock/pure tests; no fresh tests executed.
- External presence diagnostics and D0078 resolution remain valid; D0037/D0038 closed identity tradeoffs and separate request-template/local-status work must not be reopened. D0153 records future real repeated-header integration coverage; current integration files stay untouched.
- Exact technical allow-list: target, new repeated-denial-header regression knowledge, and the existing ForwardAuth knowledge index. Fresh implementer must return a plan before editing; no integration or task-file changes.

## Proposed implementation plan

- Implementer `b0290_implementation` returned a plan without edits or checks. For each first case-equivalent configured name, iterate `HeaderMap::get_all` in source order; retain current text filtering, insert the first retained value and append later retained values. Deduplicate configured names by scanning prior configuration entries without an owned collection.
- Extend the existing denial-response mock test with ordered repeated challenges/cookies, case-variant duplicate configuration, invalid text filtering, unselected exclusion and all existing status/body/framing/EOS/shutdown/log assertions. Success-side identity behavior remains untouched.
- Correct the target's selective request-header example with explicit `forwardAll: false`, whose default is true. Add the regression knowledge page and owning index link with scope limits.
- Fresh plan reviewer `b0290_plan_review` assigned. Stop before implementation if a shared mechanism, schema, another file or different error behavior is required.
- Plan reviewer `b0290_plan_review`: approved, no MustFix. Confirmed `get_all` and Pingora insert/append ordering, prior-slice case-insensitive dedup, invalid-value filtering, and `forwardAll` default. Implementation must advance insert/append state only after a successful write and retain `push_err` for either failure.

## Implementation, review and verification

- Implemented the approved three-file scope: repeated selected denial headers retain ordered separate field lines; case-equivalent configured names are processed once; invalid text filtering and both write-error diagnostics remain unchanged. The selective example now explicitly disables `forwardAll`; the new regression guard is indexed.
- Existing denial-response mock coverage now asserts two challenges, two separate cookies, config deduplication, filtering/exclusion, status/content type/connection, body pointer/length, EOS, shutdown and termination. ForwardAuth module tests: 7/7 passed.
- Fresh final reviewer `b0290_final_review`: LGTM, no MustFix or ShouldFix. Independently verified the full target, Pingora APIs, test assertions, knowledge and all hard gates.
- Root gates: format apply/check, locked workspace/all-targets check (14.06s), locked Clippy (16.30s, existing warnings, exit 0), agent-doc, SSA force, metrics inventory, Pingora policy, Gateway tracing, English-only, frozen integration and diff checks passed.
- D0153 remains the future integration owner. No integration edit. Commit `0241f37c`; no push.
