---
name: h2-grpc-sentinel-refactor-declined
description: The HTTP-status sentinel (501/503) in end_response_grpc_unimplemented/unavailable is intentional; the GrpcStatus-enum-everywhere refactor is declined. Reject re-reports.
---

# refactor-h2-grpc-02 — HTTP-status sentinel in gRPC error helpers is intentional

**Status:** ⚠️ accepted as-is (refactor declined) · 2026-05-21
**Not an RFC divergence:** the mapping `501 → grpc-status 12 UNIMPLEMENTED` (and `503 → 14 UNAVAILABLE`) is RFC-correct. This is a code-clarity finding, not a compliance gap.

## Conclusion

`end_response_grpc_unimplemented` calls `send_grpc_error_three_frame(.., 501)` and
`end_response_grpc_unavailable` calls it with `503`. These HTTP codes are *internal
sentinels* fed into the `http_to_grpc_status` mapping table — the gateway never emits
HTTP 501/503 on the wire (gRPC responses are always HTTP 200 + trailers), and the access
log records 200 (`error_response.rs` sets `ctx.request_info.status = Some(200)` before any
logging). The "log scrapers may see status=501" concern in the finding is therefore moot.

The "HTTP-status-centric" gRPC error model is a **deliberate, documented design**:
every public `end_response_*` helper takes an HTTP status and delegates to
`send_grpc_error_three_frame`, which converts via `http_to_grpc_status`. The two
gRPC-only helpers reuse that single dispatch point rather than introducing a parallel
gRPC-native path.

Suggestions of the form "introduce a typed `GrpcStatus` enum and replace the 501/503
sentinel at all callers" / "change `send_grpc_error_three_frame`'s signature to take a
gRPC status" are **not accepted**.

## Why the enum-everywhere refactor is declined

1. **Conflicts with the documented HTTP-status-centric model.** The model is recorded in
   three places: the `error_response.rs` module doc, the four-path drain note in
   `edgion-gateway/src/ctx.rs` (`pending_response_header_ops`), and an independent twin
   `send_grpc_error_three_frame` in `session_adapter.rs`. Changing the signature to a
   `GrpcStatus` enum would split the single "HTTP status is the source of truth" contract.
2. **Blast radius is ~10 call sites across 4 modules, not "1 direct caller".**
   `send_grpc_error_three_frame` / `http_to_grpc_status` serve 6 HTTP-aware helpers
   (`end_response_400/421/404/500/502/503`, each with a *genuine* HTTP status that they
   also send on the non-gRPC branch) plus `session_adapter` (2 sites), `pg_fail_to_proxy`,
   and `plugin_runtime` — all of which legitimately start from an HTTP status. Forcing
   those through an enum is actively worse.
3. **Value is clarity-only.** RFC strictness is informative (no MUST/SHOULD); real-world
   harm is negligible (sentinel never reaches wire or access log); only the two gRPC-only
   helpers carry a faintly opaque magic number. Cost/risk of the wide refactor exceeds the
   benefit.

Only `end_response_grpc_unimplemented` and `end_response_grpc_unavailable` are truly
"gRPC-only" (no HTTP response exists). If a future change wants to remove the residual
opacity, the design-respecting shape is a narrow gRPC-native writer used *only* by those
two helpers — leaving the HTTP→gRPC mapping intact for everyone else. That is an optional
polish, not a required fix.

## Code shape today

- `edgion-gateway/src/runtime/server/error_response.rs::end_response_grpc_unimplemented` — `send_grpc_error_three_frame(.., 501)`
- `edgion-gateway/src/runtime/server/error_response.rs::end_response_grpc_unavailable` — `send_grpc_error_three_frame(.., 503)`
- `edgion-gateway/src/runtime/server/error_response.rs::http_to_grpc_status` — the mapping table (`501 → (12, "unimplemented")`, `503 → (14, "unavailable")`)

## Re-evaluation triggers

- The HTTP-status-centric error model is itself replaced (then the sentinel disappears with it).
- A concrete bug is traced to the sentinel (e.g. a future caller passing the wrong code).

## Reference

- Historical verifier (source queue removed): `tasks/todo/rfc-review/_raw/verification-a2.md → N7` (CONFIRM, but RFC-CHECK = N/A clarity-only)
- Related rename: `naming-h2-grpc-01` (`send_grpc_trailers_only` → `send_grpc_error_three_frame`)
