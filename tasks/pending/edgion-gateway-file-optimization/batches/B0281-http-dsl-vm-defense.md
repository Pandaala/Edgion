# B0281 — HTTP DSL VM Defense

- Pipeline state: terminal; examined-no-change, pending local commit. No technical mutation.
- Active anchor: `edgion-gateway`; baseline `9e3a276b`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/vm_defensive_tests.rs`.
- Analyst: `b0281_vm_analysis`; examined-no-change, simple. Seven manual-bytecode tests accurately exercise VM stack/constant/regex index defenses and JSON-select construction validation. Compiler, Validator, resolve_script, execute_safe and sibling defense/property/error coverage inspected; no style-only churn justified.
- Target blob: `3eb646c13c034b24beb4544e444316710a5eb163`, unchanged at report completion.
- Separate candidate finding: deserialize knowledge still describes Regex(String) as serde-time compilation and omits existing compilation limits; independently validate after serial promotion without resurrecting the externally completed trust-boundary task.
- B0280 committed as `f8c265eb`; only campaign records/task changed since the analysis baseline. B0281 now owns the serial lane. Existing integration and user handoff/WAF paths remain protected.
- Independent reviewer: `b0281_vm_review`, LGTM; no Must Fix or disagreement. Original knowledge, full code/coverage and baseline freshness independently checked.
- Commit allow-list: ledger, main, changelog, this record, B0280 SHA backfill and independently verified standalone deferred task/backlink/index if needed. No technical or integration edits.
- D0147 independently confirmed: serde keeps Regex(String), compiler/Validator/VM have explicit 1 MiB compilation caps, and locked serde_json defaults to depth 128. Correct owning knowledge separately without reopening the completed transport/trust-boundary task.
- Add B0282 read-only candidate bookkeeping to commit scope; no next-batch implementation or review before this batch commits.

## Verification

- At f8c265eb: target tests 7/7 passed; workspace check and Clippy passed (exit 0, existing warnings).
- Formatting, format check, agent-doc validation, SSA, metrics, Pingora source policy, Gateway no-tracing and English guards passed.
- Final frozen-path and whitespace checks are mandatory before staging. Existing tests and user-owned paths remain unchanged.
