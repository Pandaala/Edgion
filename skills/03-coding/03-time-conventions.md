# Time / timestamp / duration conventions

> Canonical rules for how Edgion represents *time* in every surface: operator-facing
> durations, monotonic elapsed measurement, persisted/K8s-facing timestamps, and
> hot-path log timestamps. Four orthogonal concerns, four required shapes — they are
> not interchangeable, and none of them "should be unified" into one.

## The map: which surface → which shape

| Concern (what the value *means*) | Shape | Helper / API | Why this shape |
|----------------------------------|-------|--------------|----------------|
| **Operator-facing duration** (timeout / interval / ttl — "a length of time") | GEP-2257 string (`30s`, `5m`, `1h30m`, `500ms`) | `parse_duration()` / `GEP2257_PATTERN` (`edgion-resources/src/duration.rs`) | Gateway API standard; no bare numbers/decimals/`d` |
| **Monotonic elapsed** ("how long has passed") | `std::time::Duration` from a monotonic clock | `Instant::now().saturating_duration_since(prior)` | Wall-clock skew (NTP / clock step) makes `.elapsed()` / `.duration_since()` **panic** — a panic on the data-plane hot path is a 5xx |
| **Operator- / K8s-facing timestamp** (status `lastTransitionTime`, ACME `lastRenewalTime`/`lastFailureTime`, EndpointSlice `resolved_at` — "a point in time, persisted") | RFC3339 UTC, **second** precision, `Z`-suffixed (`2026-07-21T10:00:00Z`) | `now_rfc3339()` (`edgion-common/src/time.rs`) | Matches `metav1.Time` serialization — see below |
| **Hot-path wire-/access-log timestamp** ("a point in time, on the fast path") | Unix epoch **millis** (`i64`) | `chrono::Utc::now().timestamp_millis()` | Cheaper to encode / index per request; deliberately diverges from the RFC3339 shape |
| **Internal epoch math** (token/nonce expiry, rate-limit windows — never leaves the process) | `std::time::SystemTime` | `duration_since(UNIX_EPOCH)` — **never** a bare `.unwrap()`; use `.unwrap_or_default()` | Process-local; no serialized shape to honor |

**Rule of thumb:** `chrono` RFC3339 for anything persisted / human- / K8s-facing;
`std::time::SystemTime` only for internal epoch math that never crosses a process
boundary.

## Why operator/K8s timestamps are RFC3339 seconds + `Z` (Gateway-API alignment)

Gateway API status conditions are standard Kubernetes `metav1.Condition`, whose
`LastTransitionTime` field is `metav1.Time`. `metav1.Time` serializes as:

```go
t.UTC().Format(time.RFC3339)   // -> UTC, second precision, "Z" suffix
```

i.e. exactly `2026-07-21T10:00:00Z`. `now_rfc3339()`
(`Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)`) produces the byte-identical
shape, so Edgion's control-plane output looks like a native K8s object.

A bare `Utc::now().to_rfc3339()` yields nanosecond precision and a numeric offset
(`2026-07-21T10:00:00.123456789+00:00`). That is still valid RFC3339 and passes the
OpenAPI `format: date-time` check, **but no K8s-native object ever looks like that**,
and mixing both shapes on one field breaks string-diff / dedup logic that assumes a
single form (see the sentinel-timestamp handling in
`edgion-controller/.../resource_processor/handlers/edgion_backend_traffic_policy.rs`).
Therefore every operator/K8s-facing `now()` timestamp goes through `now_rfc3339()`.

> **`Condition.last_transition_time` is typed `String`**
> (`edgion-resources/src/resources/common/mod.rs`) — nothing enforces the shape at
> the type level, which is exactly why the single-helper discipline matters.

### Reformatting an existing `metav1.Time` is not the same as stamping `now()`

Sites that read an already-existing `creation_timestamp` / certificate `notAfter` and
call `.to_rfc3339()` on it are producing an internal change-detection **key** or a
display value, not writing a fresh operator-facing status timestamp. They are
self-consistent and are intentionally left as-is
(`edgion_backend_traffic_policy.rs`, `backend_tls_policy.rs`, ACME `certificate_not_after`).

## Do / Don't

- **Do** call `edgion_common::time::now_rfc3339()` for any persisted operator/K8s
  timestamp. There is exactly one implementation; import it, don't inline.
- **Don't** write a bare `Utc::now().to_rfc3339()` / `.to_rfc3339_opts(...)` for those
  fields.
- **Don't** hand-roll a duration parser or pull in `humantime` (not a workspace dep);
  `RateLimit` already had to be migrated back once.
- **Don't** use `.elapsed()` / `.duration_since()` for elapsed measurement — always
  `Instant::now().saturating_duration_since(prior)`.
- **Don't** convert the hot-path access-/wire-log epoch-millis timestamps to RFC3339
  "for consistency" — the divergence is intentional and performance-motivated.

## Related canon

- `skills/03-coding/02-rust-coding-rules.md` — the `Instant` / `saturating_duration_since`
  rule (monotonic elapsed) and the `unwrap` ban.
- `skills/00-design-rules.md` §I — the graded one-line index that points here.
