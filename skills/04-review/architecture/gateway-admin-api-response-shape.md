---
name: gateway-admin-api-response-shape
description: Decide on "Gateway admin API should unify response structs / return real HTTP error status codes / reject unknown kinds with 400" findings. Closed as accepted design tradeoff; do not re-file.
---

# Gateway Admin API Response Shape — Accepted Tradeoff

**Audit date**: 2026-05-27
**Linked issue**: infra-cluster-F002 (closed, not fixed)
**Conclusion**: Gateway admin API's "HTTP 200 + body-level `success`" convention and the three parallel response structs (`ApiResponse<T>` / `ListResponse` / `AccessLogListResponse`) are the current design choice. Same for `list_resources` returning `{"success":true,"data":[],"count":0}` on unknown kind names. Re-filed findings of these three forms are not accepted.

## What was filed (F-002, Low / Redundancy / API Ergonomics)

The finding flagged three coupled defects in `src/core/gateway/api/mod.rs` (+ `src/core/gateway/observe/access_log_store.rs:59-63`):

1. **Three parallel response structs**: `ApiResponse<T>` (generic, optional `data`/`error`), `ListResponse` (concrete, `success`/`data: Vec<Value>`/`count`), `AccessLogListResponse` (same shape as `ListResponse` over `AccessLogListItem`).
2. **Defect A — unknown kind returns 200 + empty list**: `list_resources` calls `ResourceKind::from_kind_name(&kind_str).unwrap_or(ResourceKind::Unspecified)`; the `list_client_resources!` macro's catch-all `_ => vec![]` yields `{"success":true,"data":[],"count":0}` — clients cannot distinguish "kind does not exist" from "kind has zero resources".
3. **Defect B — every error returns HTTP 200**: every `ApiResponse::err_body(...)` return site in this file (50+ sites across `get_resource`, all Redis/Etcd/ES testing helpers, testing handlers) returns `Json<ApiResponse<_>>` rather than `(StatusCode::XXX, Json<_>)`. Only `readiness_check` returns a real non-200 status.

Suggested fix in the issue file: collapse the three structs into `ApiResponse<Vec<Value>>`, add `(StatusCode::BAD_REQUEST, ...)` for unknown kinds, and wrap every error site with a real 4xx/5xx tuple.

## Why closed without code change

### Severity vs. blast radius

Severity is Low / Admin-only / read-dominated / **no security impact, no runtime correctness impact** — purely developer experience for tooling. The full fix is wide-impact:

- ~50+ `err_body(...)` call sites need rewriting from `Json(...)` to `(StatusCode::XXX, Json(...))`, touching all testing endpoints (`/api/v1/testing/...` Redis/Etcd/ES) plus `get_resource`.
- Deleting `ListResponse` / `AccessLogListResponse` is a public response-shape change.
- Multiple downstream consumers already parse the current shape:
  - **Dashboard** (`edgion-dashboard`) reads `success` / `data` / `count` from list endpoints.
  - **Validator binaries** (`examples/code/validator/resource_diff.rs`, `cache_residue.rs`) hit `/configclient/<kind>/list` and parse the current shape.
  - **Access-log client** (`examples/code/client/access_log_client.rs`) deserializes `AccessLogListResponse` as a named struct.
  - **10+ conf-sync / k8s shell scripts** (`examples/test/scripts/conf-sync/*.sh`, `examples/k8stest/scripts/common/*.sh`) poll `/configclient/<kind>/list` with `curl -sf`; flipping unknown-kind to 400 changes `-f`'s behaviour and any typo'd kind path (lowercase variants, hypothetical k8s plural names, etc.) starts to fail loudly instead of silently returning an empty list.

The cost/risk of the contract migration is asymmetric with a Low-severity DX defect that has **zero in-the-wild operational impact** (no incident, no report).

### Defect A is the only piece with concrete user value

Of the three sub-defects, only Defect A maps to an observable client confusion ("is the kind wrong or just empty?"). All current callers in-repo pass valid kind strings, so even the narrow fix (`(StatusCode::BAD_REQUEST, Json(...))` on `ResourceKind::Unspecified` in `list_resources` and `get_resource`) is theoretical — there is no concrete script that fails today because of the silent empty list. Keeping the narrow fix on the bench rather than landing it preserves consistency with the rest of the admin API's current "HTTP 200 + body-level success" convention.

### Defect B's "all errors as 200" is the convention, not an oversight

The admin API's intentional posture is body-level success signalling (`success: bool` + optional `error: String`), making the surface uniform whether the client is a curl-grade script or a typed Rust client. Real non-200 status is reserved for `readiness_check` (where 503 has a specific meaning to the orchestrator). Partial migration (some endpoints return real 4xx, others don't) is strictly worse than full uniformity in either direction.

### Struct unification (cosmetic) is not a defect

`ListResponse` and `AccessLogListResponse` differ from `ApiResponse<Vec<Value>>` in one concrete way: `count: usize` is a top-level field, not derived from `data.len()` by the client. The reduction is ~20 lines of struct definitions at the cost of changing two public response shapes in a way that breaks `examples/code/client/access_log_client.rs::AccessLogListResponse` deserialisation. Net negative for a Low-severity finding.

## Re-open conditions

Re-file as a **new** issue (not as a re-report of F-002) only when one of these is concretely true:

1. **A production / operability incident** traces back to "client mistook silent empty list for valid empty result" or "monitoring missed a failure because the body said `success: false` but HTTP said 200".
2. **A new external consumer** is added that has standard HTTP-error semantics (e.g. a generic Prometheus webhook, an Ops dashboard with retry-on-non-2xx baked in) and cannot reasonably adapt to body-level success.
3. **A coordinated v2 API surface** is being introduced (new path prefix, e.g. `/api/v2/...`); use the v2 surface as the migration vehicle rather than mutating `/configclient/...` in place.

Until one of those hits, re-reports of the following are **not accepted**:

- "Unify `ApiResponse<T>` / `ListResponse` / `AccessLogListResponse`."
- "Return `(StatusCode::BAD_REQUEST, ...)` instead of HTTP 200 + empty list for unknown kinds on `list_resources` / `get_resource`."
- "Wrap every `err_body(...)` return with `(StatusCode::4xx, ...)` so clients can detect failures by HTTP status."

## Related entries

- `architecture/size-and-similarity-heuristics.md` — `infra-cluster-F003` covered the "split `src/core/gateway/api/mod.rs` (1644 lines) into submodules" finding on the same file. F-002 is the response-contract counterpart of that decision: same file, same admin-API surface, same accepted-tradeoff disposition.
