# B0280 — HTTP DSL Termination Tests

- Pipeline state: terminal; deferred, committed as `f8c265eb`. No technical mutation.
- Active anchor: `edgion-gateway`; baseline `ec5eb474`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/terminate_tests.rs`.
- Analyst: `b0280_terminate_analysis`; defer to D0146. Four VM tests are accurate for the signal, but silent-close descriptions conflict with observed 502 fallback and mandatory bare-termination bug policy. No local behavior fix authorized; no unsafe continuation condition.
- Existing integration and user handoff/WAF paths remain protected; avoid deferred network unit cases.

## Review and scope

- Analyst traced parser/compiler/VM, execute_safe, request/response runtime and frozen 502/200 integration; read original design F, DSL engine/security/error, review routing and unit rules.
- Fresh `b0280_defer_review` assigned; exact target blob `fa3fd001cdb850f31350219a9b7fa53b39ae7b6f` remains unchanged.
- Preserve binary logging/access-log ownership, redaction, restraint, unified termination/failure policy, English and streaming gates. Existing code/tests are observations, not exceptions to authority.
- Commit scope: this record, ledger/main/changelog, B0279 SHA backfill, standalone D0146 task and coordinator deferred backlink/index. No code edits.

## External synchronization and refreshed review

- External history ec5eb474..9e3a276b includes a merge. Diagnostic `b0280_external_sync` verified net Gateway M13/A1: reopened ten changed and one deferred row, retained two pending rows and added one pending test file. Preserve external D0115/D0102/B0181 and D0082 retirement records.
- Frozen files verified against old Git fingerprints and new Git/worktree bytes/mode in two stages: ec5eb474..f920df56 M29/A31/D6; f920df56..9e3a276b M5/A1. Net M33/A32/D6; 3,264 entries now pass the complete guard. No integration-file writes. Add manifest to commit scope.
- Five removed DSL task backlinks retired in coordinator INDEX only; no task recreation. Inventory is 550 current Gateway files with no missing rows.
- Independent `b0280_defer_review` re-read updated DSL/plugin knowledge at 9e3a276b and reaffirmed defer, safe continuation and unchanged target blob. Updated D0146: API now describes fallback, while syntax/test prose still promises silent close; completed grpc_error documentation work is not reopened. Weaker resource-enum wording does not override Design F.
- Fresh verification at 9e3a276b passed: termination 4/4 and decision-log 6/6 tests; workspace check and Clippy (exit 0 with existing warnings); formatting and format check; agent docs, SSA, metrics, Pingora source, Gateway no-tracing, English and frozen-path guards; diff whitespace check. Clippy was rerun after its prior process result was unavailable.
- Independent review: LGTM for deferral; no unresolved Must Fix. Prior checks at f920df56 are historical only.
- Include the B0281 read-only candidate record in bookkeeping scope; no B0281 implementation or independent review started before this commit gate.
