---
name: es-bulk-max-body-bytes-enforced
description: Read when reviewing LinkSys Elasticsearch bulk findings about EsBulkConfig.max_body_bytes being configured/clamped but flush_batch never enforcing it / operator-supplied request-body ceiling silently bypassed.
knowledge_type: regression-guards
---

# LinkSys Elasticsearch bulk `max_body_bytes` enforcement (link-sys-F002)

**Status / verdict**: fixed (link-sys-F002, 2026-05-25)

When reviewing findings of the form "`EsBulkConfig.max_body_bytes` is configured/clamped but `flush_batch` never enforces it / the bulk NDJSON body is built for the whole batch with no size check / the operator-supplied request-body ceiling is silently bypassed": **fixed (link-sys-F002, 2026-05-25)**. `flush_batch` (`edgion-gateway/src/link_sys/providers/elasticsearch/bulk.rs`) now partitions the batch via `partition_into_sub_batches` (per-doc cost = `ndjson_header_line(index).len() + 2 + doc.len()`) and sends each sub-batch through the extracted `send_bulk_request`; a single doc exceeding the limit is dropped with an aggregated warning. The common in-limit case yields one sub-batch byte-identical to before. Re-reports of the "max_body_bytes not enforced" form are not accepted; verify `flush_batch` calls `partition_into_sub_batches` before re-reporting. Distinct from gateway-infra-02 (NDJSON header escaping in the same file).

## Historical resolution evidence: gateway-infra-02

ES bulk NDJSON index-action header now built via `serde_json::json!({"index":{"_index":index_name}})` + `to_string` (hoisted out of the per-doc loop, constant per batch) instead of raw `format!(r#"{{"index":{{"_index":"{}"}}}}"#, index_name)`. `build_ndjson_body` (`edgion-gateway/src/link_sys/providers/elasticsearch/bulk.rs`) escapes any `"`/`\` in the index name so the wire body stays valid NDJSON. Not remotely exploitable today (`index_name` = operator `index_prefix` + chrono date, no user input); the value was robustness against an operator typo / future dynamic index name breaking the whole batch. Output is byte-identical for valid names (no behavior change). Unit regression `test_ndjson_body_escapes_special_chars_in_index_name`. Document bodies were already safe (`AccessLogEntry::to_json()` uses serde_json). Re-reports of the raw-`format!` form are not accepted; verify the current `build_ndjson_body` uses serde_json before re-reporting.

## Resolution references

- `gateway-infra-02` — 2026-05-24.
