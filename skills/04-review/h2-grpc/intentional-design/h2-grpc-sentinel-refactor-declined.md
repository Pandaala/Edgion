---
name: h2-grpc-sentinel-refactor-declined
description: Retain the narrow 501/503 helper sentinels without forcing HTTP-derived callers through a gRPC enum; exact gRPC status writers coexist with HTTP mapping.
knowledge_type: intentional-design
---

# refactor-h2-grpc-02 — HTTP-status sentinel in gRPC error helpers is intentional

**Status:** ⚠️ accepted as-is (refactor declined) · 2026-05-21
**Not an RFC divergence:** the mapping `501 → grpc-status 12 UNIMPLEMENTED` (and `503 → 14 UNAVAILABLE`) is RFC-correct. This is a code-clarity finding, not a compliance gap.

## Conclusion

`end_response_grpc_unimplemented` calls `send_grpc_trailers_only(.., 501)` and
`end_response_grpc_unavailable` calls it with `503`. These HTTP codes are *internal
sentinels* fed into the `http_to_grpc_status` mapping table — the gateway never emits
HTTP 501/503 on these gRPC reply paths. The writer claims final downstream HTTP 200
with `ctx.claim_final_response(200)` and independently records the gRPC status.
Terminal logging finalizes serialized `request_info.status` from the actual final
downstream header or selected terminal status; the writer does not assign that
serialized field. A mapped sentinel therefore does not become access-log HTTP 501/503.

The HTTP-aware `end_response_*` helpers retain `http_to_grpc_status` mapping for
their gRPC branches. The two gRPC-only convenience helpers reuse this mapping.
This is not a universal HTTP-status source-of-truth rule: exact plugin gRPC codes
already use `send_grpc_status_trailers_only` or the adapter's
`send_grpc_error_direct`, converging on `send_grpc_trailers_only_frame`. An arbitrary
gRPC code must not be flattened through the lossy HTTP mapping.

The declined refactor is narrowly an enum-everywhere rewrite merely to remove
these two documented sentinels, not a prohibition on exact-status handling or
on fixing a demonstrated response defect.

## Why the enum-everywhere refactor is declined

1. **HTTP-derived failures still need the mapping.** Existing exact-status writers
   coexist with it, sharing terminal framing rather than duplicating protocol logic.
   Replacing the mapping everywhere provides no benefit to genuine HTTP callers.
2. **The scope is wider than the two convenience helpers.**
   `send_grpc_trailers_only` / `http_to_grpc_status` serve six HTTP-aware helpers
   (`end_response_400/421/404/500/502/503`, each with a *genuine* HTTP status that they
   also send on the non-gRPC branch), the adapter's HTTP-mapped path and
   `pg_fail_to_proxy`. These callers legitimately start from an HTTP status.
3. **Value is clarity-only.** RFC strictness is informative (no MUST/SHOULD); real-world
   harm is negligible (sentinel never reaches wire or access log); only the two gRPC-only
   helpers carry a faintly opaque magic number. Cost/risk of the wide refactor exceeds the
   benefit.

Among these convenience helpers, `end_response_grpc_unimplemented` and
`end_response_grpc_unavailable` are gRPC-only. If their residual opacity becomes
worth changing, reuse the existing exact-status writer and preserve HTTP mapping
for HTTP-derived failures. No new writer is needed; this remains optional polish.

## Code shape today

- `edgion-gateway/src/runtime/server/error_response.rs::end_response_grpc_unimplemented` — `send_grpc_trailers_only(.., 501)`
- `edgion-gateway/src/runtime/server/error_response.rs::end_response_grpc_unavailable` — `send_grpc_trailers_only(.., 503)`
- `edgion-gateway/src/runtime/server/error_response.rs::http_to_grpc_status` — the mapping table (`501 → (12, "unimplemented")`, `503 → (14, "unavailable")`)

## Re-evaluation triggers

- The HTTP-status-centric error model is itself replaced (then the sentinel disappears with it).
- A concrete bug is traced to the sentinel (e.g. a future caller passing the wrong code).

## Reference

- Historical verifier (source queue removed): `tasks/todo/rfc-review/_raw/verification-a2.md → N7` (CONFIRM, but RFC-CHECK = N/A clarity-only)
- Historical rename: `naming-h2-grpc-01` (`send_grpc_trailers_only` → `send_grpc_error_three_frame`); the three-frame implementation was subsequently replaced. Current names and framing above supersede that historical shape.

## Resolution references

- `refactor-h2-grpc-02` — 2026-05-21 (⚠️ accepted as-is, refactor declined).
