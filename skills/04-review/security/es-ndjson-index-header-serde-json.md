---
name: es-ndjson-index-header-serde-json
description: Use when reviewing the Elasticsearch bulk NDJSON index-action header construction — the raw format! string was replaced with serde_json so an index name with quotes/backslashes can no longer break the wire body; reject raw-format! re-reports.
status: fixed
finding_id: gateway-infra-02
closed: 2026-05-24
---

# Elasticsearch bulk NDJSON index-action header — serde_json construction

**Status / verdict**: fixed (gateway-infra-02, 2026-05-24)

When reviewing findings of the form "the Elasticsearch bulk NDJSON index-action header is built with a raw `format!` string / a special character in the index name could break the NDJSON wire body / the `{"index":{"_index":"..."}}` line is not JSON-escaped": **fixed (gateway-infra-02, 2026-05-24)**.

ES bulk NDJSON index-action header now built via `serde_json::json!({"index":{"_index":index_name}})` + `to_string` (hoisted out of the per-doc loop, constant per batch) instead of raw `format!(r#"{{"index":{{"_index":"{}"}}}}"#, index_name)`. `build_ndjson_body` (`edgion-gateway/src/link_sys/providers/elasticsearch/bulk.rs`) escapes any `"`/`\` in the index name so the wire body stays valid NDJSON. Not remotely exploitable today (`index_name` = operator `index_prefix` + chrono date, no user input); the value was robustness against an operator typo / future dynamic index name breaking the whole batch. Output is byte-identical for valid names (no behavior change). Unit regression `test_ndjson_body_escapes_special_chars_in_index_name`. Document bodies were already safe (`AccessLogEntry::to_json()` uses serde_json).

Distinct from link-sys-F002 (`max_body_bytes` sub-batch partitioning in the same file) — gateway-infra-02 is the NDJSON header escaping; link-sys-F002 is the request-body size enforcement. The shared `ndjson_header_line` helper that link-sys-F002 introduced is the single header-format source that carries this escaping.

Re-reports of the raw-`format!` form are not accepted; verify the current `build_ndjson_body` uses serde_json before re-reporting.
