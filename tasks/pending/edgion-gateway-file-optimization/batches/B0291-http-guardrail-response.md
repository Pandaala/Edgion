# B0291 — HTTP Guardrail Response

- Pipeline state: changed; corrected plan/final LGTM and all checks passed, local commit gate.
- Active anchor: `edgion-gateway`; baseline `99069928`.
- Target: `edgion-gateway/src/plugins/http/guardrail/response.rs`.
- Analyst: `b0291_guardrail_response_analysis`; full target, external diff, guardrail owners, streaming/RawWindow/cap/error/access-log contracts and deterministic coverage assigned. Prior B0079 evidence is historical.
- B0290 owns the serial lane. Refresh before promotion and deduplicate existing tasks/deferred findings.
- Preserve main-response streaming, canonical semantic window ownership, bounded outbound exchanges, failure policy, binary/access-log observability, redaction/restraint/unification and English-only content. No edits, tests/builds/network, integration changes/additions or protected user-path changes.
- Full target and response processor/terminal/RawWindow/Guardrail consumers examined. `emit_policy_reject` renders and terminates a non-Hold rejection without first claiming `GuardrailRejected`; the driver later backfills `LegacyProcessor`, losing the correct cause/emission and preemption reason. The Hold path already claims in the correct order.
- Proposed exact scope: target source and inline tests only. Claim before rendering; a defensive claim conflict emits no competing event and publishes one bounded conflict/superseded summary. Preserve accepted output, current-window discard, fail-open-independent rejection, streaming/backpressure, generic bounded body and exactly-once access-log summary.
- Network-free real-adapter/context test must assert Semantic winner, GuardrailRejected cause, Rendered emission, one bounded event, terminated sink and exactly one summary. Existing RawWindow strict work and D0105 multi-value Content-Encoding remain separately owned.
- External default outbound diagnostics collector is contract-correct and needs no change.
- Promoted after B0290 commit `0241f37c`; target and relevant terminal path were not changed by B0290. Fresh implementer must return a reviewed plan before editing. Exact technical allow-list remains the target and its inline tests only.

## Proposed implementation plan

- Implementer `b0291_implementation` returned a plan without edits/checks. Non-Hold rejection claims `GuardrailRejected` before rendering; success retains render, emission marking, sink termination and reject/policy summary order.
- A claim conflict returns without rendering, marking, terminating or stealing ownership and publishes only a fixed `superseded/terminal-owner-conflict` Guardrail summary.
- Two real-adapter/context, network-free inline tests cover successful Semantic ownership/rendered event/discard/accepted-output preservation/exact summary and a preclaimed-owner conflict with no competing event or sink termination.
- Fresh plan reviewer `b0291_plan_review` assigned. Stop for scope expansion, new production seams/global state, or RawWindow/driver/Content-Encoding changes.
- Initial plan review: Must Fix test boundary only. `process_batch` requires an HTTP callout and the full driver would terminate after observing an existing winner. Revised tests must directly exercise `apply_decision(Reject)` then private `emit_policy_reject` through a Semantic real adapter, inspect context after dropping it, and cover the AttachedGuardrail OnceLock first-write rule. Production claim ordering was accepted; no code edit started.
- Revised plan review: approved. Tests explicitly use the direct callback boundary with a retained generation, `apply_decision(Reject)`, real Semantic adapter and separate fresh/prior-summary conflict cases. The no-sink-termination conflict assertion is callback-local and makes no claim about the enclosing driver. Original implementer authorized within the one-file scope.

## Implementation, review and verification

- Implemented only `emit_policy_reject` and inline tests. Successful non-Hold rejection now claims `GuardrailRejected` before bounded rendering; a conflicting owner receives no competing event, emission mutation or callback-local termination. Hold and driver behavior remain unchanged.
- Two real-adapter, duplex-backed network-free tests cover discard, accepted-byte preservation, Semantic/cause/emission, bounded event/termination/summary, conflicting owner, fresh conflict summary and prior-summary first-write behavior.
- First compile found one test-local helper shadowing error; the implementer renamed that binding and reran the full focused module successfully: 17/17 passed (40.98s build, 0.05s tests).
- Fresh final reviewer `b0291_final_review`: LGTM, no MustFix/ShouldFix/deferred finding. Independently checked the full target, actual diff, owner/terminal/driver/RawWindow/rendering boundaries and test claims.
- Root gates: format apply/check, locked workspace/all-targets check (11.35s), locked Clippy (16.85s, existing warnings, exit 0), agent-doc, SSA force, metrics inventory, Pingora policy, Gateway tracing, English-only, frozen integration and diff checks passed. Commit pending; no integration edit or push.
