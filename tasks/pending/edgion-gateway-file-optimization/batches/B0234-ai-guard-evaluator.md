# B0234 — AI Guard Evaluator

## Scope

- Pipeline state: ready to commit; implementation, independent review and all gates passed.
- Active anchor: `edgion-gateway`.
- Baseline: `2466e808`; target blob unchanged after unrelated Redis and external test commits.
- Target: `edgion-gateway/src/plugins/http/ai_guard/evaluator.rs`.
- Inspect the complete evaluator, configuration/compilation/runtime callers, request-body ownership, outbound calls and tests. Read scope may cover the cohesive AI Guard module; write recommendations must remain minimal and evidence-based.
- Root owns Cargo and all writes. Existing integrations, external todos and handoff/WAF paths remain protected. Do not promote before B0232 and B0233 complete their serial commit gates.

## Analysis Outcome

- Analyst `b0234_ai_guard_evaluator_analysis` recommends a simple one-file change at `55bb2c84`; target blob `26b393e074aec6d167f42e400213242c3add2366`. `decide` deep-clones every selected span String before capped JSON serialization, adding up to 512 allocations and up to the 8 MiB total span cap even when the evaluator's 1 MiB request cap later rejects it.
- Exact candidate scope: target only. Make the serialization-only RequestSpan borrow `&str`, project selected current text with `as_str`, and retain owned wire conversion. Preserve exact JSON bytes/fields, both caps, strict parsing, semaphore/deadline order, canonical managed outbound client/egress/TLS, bounded response, static errors and failure policy.
- Update inline encoding tests for borrowed inputs; run evaluator 6-test filter and broader AI Guard tests. No integration change or gap, new logs/metrics, schema or knowledge change.
- Analyst noted a separate user-document Bounds table discrepancy for `maxSpanBytes` versus code and earlier prose. Do not mix it into this runtime batch; validate independently before creating any task.
- Original AI/plugin/body-storage/outbound/egress/failure/observability/Rust/testing and review knowledge were independently applied. Canonical BodyStorage and capped streaming outbound response remain unchanged.

## Implementation and Review

- Implementer `b0234_implementation` made the serialization-only RequestSpan borrow `&str`, propagated the lifetime through DecisionRequest/encode_request and projected selected text with `as_str`. Two encoding tests now use borrowed inputs. WirePart, JSON representation, caps and all outbound/failure behavior remain unchanged.
- Fresh independent reviewer `b0204_plan_review`: LGTM, no Must Fix or Should Fix. Reviewed target blob `675e9560`; serialization synchronously produces owned Bytes before any permit/network await, so no new borrow crosses asynchronous ownership. Accurate benefit: one String allocation/full copy removed per nonempty evaluator span; outer payload Vec, encoded Bytes and the B0236-owned `apply_local` copies remain.
- Fresh tests: evaluator 6/6 and full AI Guard 30/30. Workspace format/check/Clippy, agent docs, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff and frozen-path checks passed.
- No integration, documentation, issue, schema, logging or metrics change. Commit: `899436e7`; never pushed by the campaign.
