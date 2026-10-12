---
name: elasticsearch-date-pattern-admission
description: Use when reviewing Elasticsearch UTC index date patterns, candidate reuse, or bulk formatter failure handling.
knowledge_type: regression-guards
finding_id: ARCH-025
---

# Elasticsearch date patterns are validated before runtime reuse

## Conclusion and scope

`ElasticsearchClientConfig::validate_date_pattern` in resources owns the pure UTC
formatting contract for explicit `index.datePattern`. `LinkSys::validate_config`
uses it for Controller feedback. Gateway `EsCandidate::from_prepared` uses it
before fingerprint/resourceVersion reuse, construction and initialization, including
the actual-owner `EsLinkClient::from_link_sys` entry point. Omitted patterns use
the established default; empty/literal/escaped-percent patterns remain accepted.

`EsBulkConfig::current_index_name` returns a Result and writes into its final String.
An unexpected formatting failure drops only the current owned flush batch before
any ES request or retry, reports a fixed background error with connector identity
and document count, and leaves the bulk worker running. It does not update transport
health or change health-based fallback. This is not general index-name validation,
delivery acknowledgement, or recovery of already-queued documents.

## Rationale and evidence

ARCH-025 found that unvalidated patterns reached Chrono formatting followed by
`to_string` on first nonempty flush, allowing a configuration error to panic and
terminate the worker. Chrono 0.4.45 parses `%#z` but cannot format it, so checking
only strftime syntax misses this case. The canonical validator formats a fixed UTC
timestamp through fallible `DelayedFormat::write_to` into a discard `fmt::Write`
sink. It neither echoes operator input nor constructs generated validation output.

Gateway admission precedes the persistent runner's reuse/build decision. Ordinary
preparation failure retains its existing last-good behavior; forced rebuild retains
its existing pre-retirement semantics. TLS, auth, fingerprints, publication,
retirement and shutdown ownership remain unchanged. Validation cost depends on
configuration processing and pattern length; no per-request/per-document check,
parsed-item cache or new telemetry is added. Direct runtime writing removes the
previous intermediate suffix/Display-output construction structurally; there is
no measured throughput, allocation-count or memory improvement claim.

## Alternatives and limits

Controller-only validation is advisory and cannot protect a direct Gateway input.
Gateway-only validation loses shared status feedback. Parser-only validation is
insufficient for output capability. Changing String output to bytes does not fix
invalid formatting instructions. Silent default substitution conceals configuration
errors. The fallible runtime guard protects the worker without catch_unwind or
redesigning health/fallback; losing the current batch is an explicitly accepted
local failure boundary.

## Verification and re-evaluation

Focused tests in resources `tests/unit/resources/link_sys/{elasticsearch,mod}_test.rs`
cover valid UTC formats, defaults and fixed errors. Gateway Elasticsearch
`{apply,config_mapping}_test.rs` covers same-resourceVersion rejection, shared
validation/constructor enforcement, effective fingerprint identity and output parity.
`bulk_test.rs` exercises the actual worker with an invalid-URL sentinel that cannot
dial, scoped diagnostics and deterministic future polling: current-batch drop,
later-batch processing, channel close, buffered shutdown and unchanged health.
Existing NDJSON and retry tests retain their identities. These are network-free
failure regressions, not evidence of live Elasticsearch delivery or every scheduler
order. Fresh executed checks are recorded in the implementation task after execution.

Re-evaluate if Chrono formatting capabilities, time-zone ownership, candidate
admission ordering, bulk failure policy or fallback semantics change. No assumption
that historical test results establish the current source is required.

## Resolution references

- ARCH-025 — Elasticsearch date-pattern cold validation and fallible runtime defense.
- `edgion-resources/src/resources/link_sys/elasticsearch.rs`
- `edgion-gateway/src/link_sys/providers/elasticsearch/{apply,config_mapping,bulk}.rs`
- `skills/01-architecture/02-gateway/09-link-sys.md`
