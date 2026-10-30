---
name: elasticsearch-health-response-bound
description: Use when reviewing Elasticsearch initialization health response reception and overflow handling.
knowledge_type: regression-guards
---

# Elasticsearch health responses are bounded before parsing

## Conclusion and scope

`EsLinkClient::cluster_health_at` uses the canonical Gateway
`util/http_body.rs::stream_body_chunks_capped` collector with a fixed 1 MiB
receiving limit. Declared oversize fails before polling. Missing or inaccurate
lengths cannot bypass the actual-byte limit. Overflow stops polling and fails
without parsing a truncated prefix; read and parse failures use fixed diagnostics.

The limit covers initialization, rebuild and explicit health reads. It does not
change `EsLinkClient` fields, endpoint selection, client/TLS/credential policy,
request timeouts, or runner cancellation. Initialization sets `healthy` from the
check result and still starts background recovery after failure. The runner can
cancel an initializing generation. Background probes discard bodies and remain
status-only; bulk acknowledgements retain their separate configured cap.

## Rationale and evidence

ES-01 identified `PersistentRunner<EsApply>` -> `EsLinkClient::init` ->
`cluster_health` -> `cluster_health_at` -> `Response::json` as a production-reachable
unbounded collector. A configured faulty endpoint could send arbitrarily large
bodies before parsing, even invalid JSON. Timeouts and HTTP/2 header bounds do
not bound response bytes. Shared collection follows the reuse and bounded-memory
rules in `skills/00-design-rules.md`; see also
`skills/04-review/cpu-memory/review-evidence/webhook-read-then-truncate-16kb-cap.md`.

## Alternatives and limits

The cluster-level health document only supplies a status string; a fixed 1 MiB
limit provides generous headroom without a new configuration field. This is a
logical input bound, not an exact allocation/RSS guarantee: collector capacity,
delivered chunks, transport buffers and the JSON tree also use memory.
There is no demonstrated anonymous downstream-request exploit or periodic
probe-body leak. Other retained CRUD/search responses remain deferred in
[ES-02](../../../../tasks/todo/elasticsearch-operation-response-caps.md).

## Verification and re-evaluation

Network-free regressions in
`edgion-gateway/tests/unit/link_sys/providers/elasticsearch/ops_test.rs` cover
normal and exact-limit documents, declared oversize without polling, absent and
inaccurate lengths, overflow before JSON parsing, stop-polling behavior, read and
parse errors, missing status, cancellation drop and a subsequent successful read.
These do not constitute live HTTP or end-to-end recovery verification.
Fresh validation passed on 2026-10-30:

- `cargo test -p edgion-gateway --lib link_sys::providers::elasticsearch -- --nocapture`: 66 passed.
- `cargo test -p edgion-gateway --lib link_sys::runtime::health_probe -- --nocapture`: 12 passed.
- `cargo test -p edgion-gateway --lib util::http_body -- --nocapture`: 5 passed.
- `cargo fmt --all` and `cargo fmt --all -- --check`.
- `cargo check --workspace --all-targets` and `cargo clippy --workspace --all-targets`.
  Clippy reported warnings in unchanged tests and integration-harness files;
  none originated in this change.
- Agent-document, SSA-force, metrics-inventory, Pingora-source-policy,
  Gateway API transform-contract and unit-test-layout guards.

Independent read-only review returned LGTM. Existing health-probe tests exercise
recovery transitions and cancellation; initialization wiring was verified by source
inspection. No full-workspace test run, live HTTP reproduction or RSS measurement
was performed.

Re-evaluate if health query detail expands, a caller bypasses the capped reader,
overflow becomes successful truncation, or health lifecycle ownership changes.

## Resolution references

- ES-01 — initialization response collection without a receiving-time byte cap.
