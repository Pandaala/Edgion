---
name: ipv6-probe-url-brackets
description: Active health probes now build the target via SocketAddr Display so IPv6 literals are bracketed, and probe_grpc no longer panics on a bad URI. Reject re-reports claiming IPv6 backends fail every probe or panic the health-check task.
---

# backends-F001 — IPv6 health-probe URLs are bracketed; gRPC probe no longer panics

**Status:** ✅ fixed · 2026-05-20
**Severity:** High (BadLogic) — IPv6 backends were always unhealthy; the gRPC probe panicked the per-service health-check task

## Conclusion

`probe_http_with_client` and `probe_grpc` (`src/core/gateway/backends/health/check/probes.rs`)
now construct the target with `SocketAddr::new(addr.ip(), port)` and rely on `SocketAddr`'s
`Display` impl, which brackets IPv6 literals (`[::1]:8080`). `probe_grpc` additionally replaced
`.parse().expect("valid URI from SocketAddr")` with `.parse().map_err(|_| ProbeError::InvalidUri)?`,
so a non-parseable URI returns an error instead of panicking the health-check background task.

Re-reports of the form "IPv6 backend probe builds `http://::1:8080/path` (missing brackets)",
"IPv6 backend is always marked unhealthy", or "`probe_grpc` `.expect()` panics on IPv6" are
**not accepted** — none are true anymore.

## What was wrong (before the fix)

Both probes used `format!("http://{}:{}", addr.ip(), port, ...)`. For an `IpAddr::V6`,
`addr.ip()` renders as `::1` with no brackets, producing the RFC-3986-invalid
`http://::1:8080/path`. The HTTP path surfaced this as a `reqwest` parse error
(`ProbeError::RequestFailed`) — every IPv6 backend was permanently unhealthy. The gRPC path
called `.expect()` on the parse result, panicking the per-service `health_check_loop` task and
silently stopping health monitoring for all backends of that service.

## Why the fix is correct

`std::net::SocketAddr`'s `Display` is the canonical, RFC-3986-correct rendering for both
families (`127.0.0.1:8080` and `[::1]:8080`). `probe_tcp` already used the same
`SocketAddr::new(addr.ip(), port)` pattern, so the fix aligns the three probe paths.
`ProbeError` is consumed only via `result.is_ok()` + `Debug` in `manager.rs` (no external
exhaustive match), so adding the `InvalidUri` variant is contract-safe.

## Fix Suggestions Not Accepted

- "Branch on `addr.is_ipv6()` to conditionally add brackets" — redundant; `SocketAddr` Display
  already handles both families.
- "Keep `.expect()` since a SocketAddr-derived URI is always valid" — defense in depth; a panic
  on the health-check task is never an acceptable failure mode.

## Re-evaluation Triggers

Re-open only if:

- The probe URL stops being derived from a `SocketAddr` (e.g., switches to a hostname-based target).

## Reference Cases

- backends-F001 (review finding, 2026-05-20)
- `src/core/gateway/backends/discovery/endpoint_slice/discovery_impl.rs:38` — the pre-existing
  correct `format!("[{}]:{}", address, port)` IPv6 pattern already in the codebase.
