# WAF default body-processor reuse

Status: deferred transaction representation and accounting decision for the user.
Source baseline: `ddea4ef89`. No runtime or dependency change.

## Evidence

Gateway WafPlugin constructs a Transaction before executing request-header rules.
In edgion-waf/src/seclang/transaction.rs, RequestState::build already computes
the first Content-Type's default processor in RequestHeadProjection and reuses
that enum while constructing the REQBODY_PROCESSOR scalar. That earlier
optimization remains constructor-local; RequestState does not retain the enum.

After phase one, Gateway may call preflight_request_body(Auto). The method in
json_inspection_runtime.rs first checks sticky health, phase order and absence
of an already published body. It then applies tx.request_body_processor over
the caller's requested selection. Only an effective Auto scans RequestHeaders
through first_named and calls body.rs::processor_from_content_type again.
The classifier borrows slices and allocates nothing. This is repeated request
metadata lookup/classification, not repeated rule compilation or a payload copy.

Subsequent Gateway paths often pass explicit resolved JSON/XML selections, so
not every preflight repeats the Content-Type lookup. The actual benefit depends
on header count, selection mode and call path; none has been benchmarked.

## Decision

Keep the current lookup, retain the constructor's default enum in transaction
request state, or reuse the existing scalar representation through one canonical
mapping. A retained enum adds per-transaction representation and may affect layout;
scalar reuse retains lookup/decoding work and must not introduce divergent media
classification. Assess actual layout and the logical retained-byte ledger instead
of assuming either a free field or an existing accounting violation.

Only the immutable default is eligible for reuse. The effective processor must
still resolve the explicit request and successful phase-one control override at
the original boundary. Do not cache the final processor before phase-one rules,
skip preflight validation, or treat failed-chain staged controls as committed.
Request-derived Content-Type cannot be compiled as one configuration-wide value.

## Acceptance for an approved implementation

Preserve first duplicate Content-Type precedence, missing-header Raw selection,
ASCII trimming/case handling, parameter splitting and existing +json/+xml suffix
classification. Preserve explicit requested processors and the transaction control
override precedence. Existing phase-one REQBODY_PROCESSOR visibility, publication
metadata and body-control behavior must remain consistent with the chosen default.

Keep sticky health, phase-order and already-published checks in their current
order. Keep raw/full/prefix admission, source limits, JSON/XML publication, chain
rollback and request isolation. Do not alter collection entry counts, projected
arena bytes, scalar payloads, generation budgets or error precedence to conceal
the representation's cost. Review fixed state layout and logical heap charges
separately; no new body cache or second processor classifier is warranted.

Compare headers with the first Content-Type early/late/absent, duplicates, supported
and unknown media types, explicit selections, successful and failed ctl chains,
repeated preflight, wrong phase and already-published calls. Retain exact budget
boundary tests and compare resulting scalar collections and selected processor.
Measure lookup work and retained state separately before claiming performance.

The anchor remains edgion-gateway. Any future implementation in the shared WAF
leaf requires an exact reviewed supporting-file allowlist and IMPL plan/final
reviews; this task grants no leaf-wide cleanup authority. Keep integration fixtures
unchanged and add no request tracing, header payload logs or metrics.

## Knowledge and verification limits

- [Request-scoped control actions](../../skills/04-review/waf/regression-guards/request-scoped-control-actions.md)
- [JSON inspection ownership](../../skills/04-review/waf/regression-guards/fused-json-inspection.md)
- [Named generation publication](../../skills/04-review/waf/regression-guards/named-profile-selection-and-bundle-publication.md)

Existing tests validate the current implementation. This audit did not implement
default reuse, measure transaction layout changes or run live Gateway benchmarks.
