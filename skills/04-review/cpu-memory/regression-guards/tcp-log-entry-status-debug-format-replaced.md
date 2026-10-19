---
name: tcp-log-entry-status-debug-format-replaced
description: TCP log status uses explicit static labels; the entry borrows connection strings and only the serialized JSON String crosses the asynchronous sender boundary.
knowledge_type: regression-guards
---

# TCP log status labels and borrowed projection

## Current contract

`TcpStatus::as_str()` owns the explicit static label table. `TcpLogEntry<'a>`
borrows the direct-peer IP, remote IP and optional upstream address from its
`TcpContext`, and retains the static status label without allocating a copy.
The deny reason remains an optional bounded static string. Existing serde
masking, omitted fields, timestamp and duration sampling remain unchanged.

The sole production builder is `EdgionTcpProxy::log_connection`. Its context
outlives the entry and the awaited `log_tcp` call. `log_tcp` synchronously
serializes the entry to an owned JSON `String` before handing that string to
`AccessLogger::send`. The entry itself does not cross a channel or require a
static lifetime. Do not reintroduce owned projection strings merely because
the sender is asynchronous.

## Historical correction

The observe-F012 change (2026-05-28) replaced Debug formatting with
`ctx.status.as_str().to_owned()`. Its lasting benefit was making the log label
contract explicit instead of deriving it from enum Debug output. It still
allocated the status String and copied address fields.

The previous version of this page incorrectly justified owned fields by
claiming that `TcpLogEntry` crossed the asynchronous channel. Current caller and
sender inspection disproves that claim: the channel receives serialized JSON.
That historical rationale must not suppress the borrowed projection change.
The earlier hard-coded variant count is also historical; the exhaustive match
and `tcp_status_as_str_matches_log_field` test define the current label set.

## Verification and re-evaluation

`tests/unit/observe/logs/tcp_log_test.rs` checks status labels, peer provenance,
deny omission, JSON equivalence and allocation-free context projection with
borrowed pointer identity. The allocation assertion excludes serialization:
JSON creation and configured redaction may still allocate.

Re-evaluate the lifetime boundary if a future sender accepts an entry instead
of serialized JSON, or if a consumer needs to retain it beyond its context.
Preserve label strings independently of enum renames, canonical masking and
bounded denial diagnostics. This change makes no throughput claim.

## Sources

- `edgion-gateway/src/observe/logs/tcp_log.rs`
- `edgion-gateway/src/routes/tcp/edgion_tcp.rs`
- `edgion-gateway/src/observe/access_log/logger.rs`
- Historical finding: observe-F012, fixed 2026-05-28.
