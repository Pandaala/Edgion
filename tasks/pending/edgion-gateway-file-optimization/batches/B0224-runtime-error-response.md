# B0224 — Runtime Error Response

## Scope

- Pipeline state: terminal; local commit `3786c2cd`.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/runtime/server/error_response.rs`.
- Baseline: `72454f81`.
- Analyst: `b0224_error_response_analysis`.
- Read complete response construction, actual HTTP/gRPC callers, final-status/log ownership and transport/unit-test contracts before assessing local value.
- No writes authorized. Existing integration files, external todos and user-owned handoff/WAF paths remain protected. New integration needs are deferred.

## Analysis Outcome

- Analyst recommends change at `72454f81`; target blob `0b62747d3b7f98545f5606e8c892e3ca2f04fb8f`. Plan-first because response delivery is touched.
- Six fixed HTML pages currently allocate/format a body String on HTTP rejection. A small compile-time literal template plus `Bytes::from_static` can remove that work; failure-path-only benefit, no healthy-traffic/QPS claim. Decline if the implementation needs elaborate abstraction.
- Exact proposed technical scope: target; `skills/04-review/h2-grpc/regression-guards/grpc-message-encoding.md`; `skills/04-review/h2-grpc/intentional-design/h2-grpc-sentinel-refactor-declined.md`. Preserve all nine tests; cover six exact bodies and first-owner behavior with existing duplex fixtures. Preserve bytes/status/headers/claim-before-write/EOS/error propagation/H1-H2 shutdown.
- Correct encoder and UTF-8 test comments only: spaces remain unescaped, percent is encoded, current accented/Greek fixtures are two-byte. Correct removed helper/old status ownership claims in the two owning knowledge pages while retaining their valid narrow decisions; existing historical three-frame banner needs no broad rewrite.
- D0126 records independently repairable request-stage queued gRPC status/message conflicts at the shared terminal writer. Source reachability verified across DSL, adapter, dispatch and installed Pingora bridge; not a fresh wire reproduction or client-specific interpretation claim. The canonical task is self-contained; do not mix that repair into HTML optimization.

## Promotion

- Refreshed mechanically at `4c575bee`: no source/skills/rules diff since analyst baseline `72454f81`; target blob unchanged. Integration freeze passed.
- Exact technical allow-list remains the target and two owning knowledge pages above. Protected user untracked handoff/WAF paths unchanged; queued D0126 record/task are campaign-owned, not external work.
- Coordinator commit scope: main task, ledger, changelog, B0223 SHA backfill, B0224/B0225/B0226 records, local deferred index/D0126 and standalone terminal-header task. No integration or shared global index edits.

## Implementation Plan

- Implementer `b0224_implementation`, complex response boundary. Fresh independent `b0224_plan_review` Approved at `4c575bee`; implementation authorized for the unchanged six-step plan. Tests assert selected status without prematurely finalizing serialized access-log status.
1. Define a private literal-only compile-time HTML template preserving every emitted byte.
2. Pass static bytes into the private HTML sender and use `Bytes::from_static`; update six HTTP branches only, preserving first-owner, headers, EOS, errors and protocol shutdown behavior.
3. Retain nine tests; add six-case public-helper exact wire/body coverage plus a closed-duplex first-owner no-op test using existing fixtures and bounded reads.
4. Correct encoding/UTF-8 comments without changing encoding behavior or expectations.
5. Update only the two owning knowledge pages for current writer, encoding and terminal-status ownership; retain scoped historical decisions and do not imply D0126 is fixed.
6. Independently review and run error-response/final-downstream-status filters plus required gates. Stop for overlap, semantic/scope changes or nonlocal test infrastructure; recover only owned hunks.

## Implementation Result

- Implementer completed the exact three technical paths. Six HTTP error bodies use compile-time literal bytes; runtime gRPC branches and existing delivery/shutdown/ownership behavior remain unchanged. Encoder/test comments and two owning knowledge pages corrected.
- All nine existing tests retained; added `html_error_helpers_preserve_exact_bodies_headers_and_statuses` and `html_error_helper_preserves_existing_response_owner` with independent exact body expectations, selected-versus-finalized status and bounded in-memory transport assertions.
- Mechanical containment, rustfmt/diff and integration freeze passed. Fresh independent final reviewer `b0224_final_review` checks both implementation and separate D0126 classification; coordinator Cargo verification is running.

## Independent Review and Verification

- Final reviewer LGTM; no Must Fix/Should Fix. Full current source, actual callers, exact body/ownership semantics and both knowledge conclusions checked. D0126 independently confirmed as a pre-existing source-reachable defect with no client-specific interpretation or fresh reproduction claim.
- Fresh error-response tests 11/11 and final-downstream-status tests 8/8 passed. Workspace format/check/Clippy (existing warnings), docs, SSA, metrics, Pingora source, Gateway tracing, English-only, diff and integration freeze passed. Logs: `/tmp/edgion-b0224-{error-response,final-status,check,clippy}.log`.
- Final source blob `b749f823461f9740e8bfc4df0d6f82aee7a326c5`. No integration changes; existing nine tests retained. Ledger/changelog updated; commit `3786c2cd`, local only.
