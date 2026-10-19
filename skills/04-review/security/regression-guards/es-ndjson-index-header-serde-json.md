---
name: es-ndjson-index-header-serde-json
description: Use when reviewing the Elasticsearch bulk NDJSON index-action header construction — the raw format! string was replaced with serde_json so an index name with quotes/backslashes can no longer break the wire body; reject raw-format! re-reports.
status: fixed
finding_id: gateway-infra-02
closed: 2026-05-24
knowledge_type: regression-guards
---

# Elasticsearch bulk NDJSON index-action header — serde_json construction

**Status / verdict**: fixed (gateway-infra-02, 2026-05-24)

When reviewing findings of the form "the Elasticsearch bulk NDJSON index-action header is built with a raw `format!` string / a special character in the index name could break the NDJSON wire body / the `{"index":{"_index":"..."}}` line is not JSON-escaped": **fixed (gateway-infra-02, 2026-05-24)**.

ES bulk NDJSON index-action header now uses borrowed typed `IndexAction` / `IndexMetadata` serialization through `serde_json::to_string` (hoisted out of the per-doc loop, constant per batch) instead of raw `format!(r#"{{"index":{{"_index":"{}"}}}}"#, index_name)`. `build_ndjson_body` (`edgion-gateway/src/link_sys/providers/elasticsearch/bulk.rs`) escapes any `"`/`\` in the index name so the wire body stays valid NDJSON. Not remotely exploitable today (`index_name` = operator `index_prefix` + chrono date, no user input); the value was robustness against an operator typo / future dynamic index name breaking the whole batch. Output is byte-identical for valid names (no behavior change). Unit regression `test_ndjson_body_escapes_special_chars_in_index_name`. Document bodies were already safe (`AccessLogEntry::to_json()` uses serde_json).

Distinct from link-sys-F002 (`max_body_bytes` sub-batch partitioning in the same file) — gateway-infra-02 is the NDJSON header escaping; link-sys-F002 is the request-body size enforcement. The shared `ndjson_header_line` helper that link-sys-F002 introduced is the single header-format source that carries this escaping.

Re-reports of the raw-`format!` form are not accepted; verify the current `build_ndjson_body` uses serde_json before re-reporting.

The original escaping fix used a temporary serde_json Value tree. The current
helper borrows the index name in two single-field structs, removing that tree
without changing keys, order, escaping or the serialized String owner. The same
helper still supplies partition byte overhead and request headers. The mirrored
`ndjson_header_matches_value_serialization_with_less_temporary_storage` regression
compares exact bytes and round-trip names with the former Value implementation
for empty, quoted, escaped, control-character, UTF-8 and long inputs. Its allocation
counter covers header construction only; it does not measure worker throughput,
whole-request allocation, live delivery or partition/retry changes.

## Historical resolution evidence: link-sys-F002

`EsBulkConfig.max_body_bytes` (default 10MB, ceiling 50MB) was parsed and stored but **never enforced** — `flush_batch` (`edgion-gateway/src/link_sys/providers/elasticsearch/bulk.rs`) built one NDJSON body for the whole batch and posted it unconditionally, so the operator-configured ceiling was silently inert (a large batch could exceed ES `http.max_content_length` → reset → retry-loop batch loss). Fixed: `flush_batch` now partitions docs via the pure helper `partition_into_sub_batches(docs, per_doc_overhead, max_body_bytes)` (per-doc wire cost = `ndjson_header_line(index).len() + 2 + doc.len()`) and sends each sub-batch through the extracted `send_bulk_request` (same retry/backoff/healthy logic, now keyed on a `doc_count` param). A single doc that alone exceeds the limit is dropped with one aggregated warning (can never be batched safely). Common case (batch within limit) → exactly one sub-batch, byte-identical to before (no behavior change). `build_ndjson_body` generalized to `<S: AsRef<str>>` to accept `&[&str]` sub-batches; `ndjson_header_line` extracted as the single header-format source (shared with gateway-infra-02's escaping). Unit regressions `test_partition_{single_sub_batch_when_within_limit,splits_on_byte_budget,drops_oversized_single_doc,empty_input,doc_exactly_at_limit_is_kept}`; integration `run_es_test.sh` CRD now sets `maxBodyBytes: 256` so the 10-doc bulk scenario splits and its count check guards against split-path data loss. Re-reports of the "max_body_bytes configured but not enforced" form are not accepted; verify `flush_batch` calls `partition_into_sub_batches` before re-reporting.

## Resolution references

- `link-sys-F002` — 2026-05-25.
