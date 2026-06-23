---
name: es-bulk-max-body-bytes-enforced
description: Read when reviewing LinkSys Elasticsearch bulk findings about EsBulkConfig.max_body_bytes being configured/clamped but flush_batch never enforcing it / operator-supplied request-body ceiling silently bypassed.
---

# LinkSys Elasticsearch bulk `max_body_bytes` enforcement (link-sys-F002)

**Status / verdict**: fixed (link-sys-F002, 2026-05-25)

When reviewing findings of the form "`EsBulkConfig.max_body_bytes` is configured/clamped but `flush_batch` never enforces it / the bulk NDJSON body is built for the whole batch with no size check / the operator-supplied request-body ceiling is silently bypassed": **fixed (link-sys-F002, 2026-05-25)**. `flush_batch` (`edgion-gateway/src/link_sys/providers/elasticsearch/bulk.rs`) now partitions the batch via `partition_into_sub_batches` (per-doc cost = `ndjson_header_line(index).len() + 2 + doc.len()`) and sends each sub-batch through the extracted `send_bulk_request`; a single doc exceeding the limit is dropped with an aggregated warning. The common in-limit case yields one sub-batch byte-identical to before. Re-reports of the "max_body_bytes not enforced" form are not accepted; verify `flush_batch` calls `partition_into_sub_batches` before re-reporting. Distinct from gateway-infra-02 (NDJSON header escaping in the same file).
