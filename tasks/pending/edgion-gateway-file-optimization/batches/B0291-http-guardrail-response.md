# B0291 — HTTP Guardrail Response

- Pipeline state: analysis-ready; change candidate, complex / plan-first; no mutation authorized.
- Active anchor: `edgion-gateway`; baseline `99069928`.
- Target: `edgion-gateway/src/plugins/http/guardrail/response.rs`.
- Analyst: `b0291_guardrail_response_analysis`; full target, external diff, guardrail owners, streaming/RawWindow/cap/error/access-log contracts and deterministic coverage assigned. Prior B0079 evidence is historical.
- B0290 owns the serial lane. Refresh before promotion and deduplicate existing tasks/deferred findings.
- Preserve main-response streaming, canonical semantic window ownership, bounded outbound exchanges, failure policy, binary/access-log observability, redaction/restraint/unification and English-only content. No edits, tests/builds/network, integration changes/additions or protected user-path changes.
- Full target and response processor/terminal/RawWindow/Guardrail consumers examined. `emit_policy_reject` renders and terminates a non-Hold rejection without first claiming `GuardrailRejected`; the driver later backfills `LegacyProcessor`, losing the correct cause/emission and preemption reason. The Hold path already claims in the correct order.
- Proposed exact scope: target source and inline tests only. Claim before rendering; a defensive claim conflict emits no competing event and publishes one bounded conflict/superseded summary. Preserve accepted output, current-window discard, fail-open-independent rejection, streaming/backpressure, generic bounded body and exactly-once access-log summary.
- Network-free real-adapter/context test must assert Semantic winner, GuardrailRejected cause, Rendered emission, one bounded event, terminated sink and exactly one summary. Existing RawWindow strict work and D0105 multi-value Content-Encoding remain separately owned.
- External default outbound diagnostics collector is contract-correct and needs no change.
