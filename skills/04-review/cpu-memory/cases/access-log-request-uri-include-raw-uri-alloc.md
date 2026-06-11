---
name: access-log-request-uri-include-raw-uri-alloc
description: Use when reviewing findings that flag the per-request String allocation in AccessLogEntry::from_context for the request_uri field (under include_raw_uri=true) and propose precomputing entry_raw_uri in RequestInfo / borrowing it as Option<&'a str>.
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index, and the sibling
> entry [access-log-json-serialization-per-request.md](./access-log-json-serialization-per-request.md)
> that closes the same category for the entry-level `to_json()` allocation.

# AccessLogEntry::from_context Per-Request request_uri String Alloc (include_raw_uri=true) is Not Worth Fixing

**False-positive scenario**: `src/core/gateway/observe/access_log/entry.rs:75-81` —
when `[access_log_options] include_raw_uri = true`, `AccessLogEntry::from_context`
calls `ctx.request_uri()`, which delegates to `compose_uri()` in
`src/core/gateway/ctx.rs:1088-1133` and always returns a freshly allocated `String`
(`format!("{}?{}", path, q)` or `path.to_string()`). Every other field on
`AccessLogEntry<'a>` is a `&'a` borrow, so reviewers see this as the lone
heap-allocating field that "breaks the zero-copy invariant" and propose adding
`RequestInfo::entry_raw_uri: Option<String>`, populating it once in
`pg_request_filter`, switching `request_uri()` to `Option<&str>`, and changing
`AccessLogEntry.request_uri` to `Option<&'a str>`.

**Reality**:

1. **The dominant per-request allocation is the entry itself, not this field.**
   Immediately after `from_context`, the access-log pipeline calls
   `AccessLogEntry::to_json()` which `serde_json::to_string(self)`s the whole
   entry into a fresh owned `String` of ~1–5 KB. That allocation is the
   accepted channel-boundary contract per
   [access-log-json-serialization-per-request.md](./access-log-json-serialization-per-request.md).
   Eliminating one ~few-dozen-byte URI string in the shadow of a 1–5 KB
   to_json() String is invisible in any profile.

2. **The proposed fix does not actually save an allocation.** Composing
   `entry_raw_uri` eagerly in `pg_request_filter` still produces one owned
   `String` per request when `include_raw_uri=true` — just at a different
   point in the lifecycle. Net per-request alloc count: unchanged. If the
   precompute is made unconditional (to keep the field simple), it becomes
   a regression for `include_raw_uri=false` deployments.

3. **The DSL/plugin path cannot benefit.** `request_uri()` has a second
   caller at `src/core/gateway/plugins/runtime/session_adapter.rs:259-261`
   exposing `fn get_request_uri(&self) -> String` to the plugin runtime
   trait. The trait boundary owns the `String`, so even if `ctx.request_uri()`
   were switched to `Option<&str>`, this site would still `to_string()` back
   into an owned `String`. Only the access-log branch could borrow, and that
   branch is already dwarfed by `to_json()` (point 1).

4. **Cost/risk of the change is not trivial.** Adding `entry_raw_uri` to
   `RequestInfo` plus threading `'a` through `AccessLogEntry.request_uri`
   touches: the `RequestInfo` struct definition, `pg_request_filter` entry
   logic, `request_uri()` signature + all callers, `session_adapter` trait
   impl, and the `AccessLogEntry` serialization (including the
   `masked_opt::serialize` adapter currently keyed on `Option<String>`).
   Mechanical, but medium blast radius for negligible measurable benefit.

**Verdict**: not a performance issue. Same category as
[access-log-json-serialization-per-request.md](./access-log-json-serialization-per-request.md)
and [oidc-jwt-claims-json-serialization-opt-in.md](./oidc-jwt-claims-json-serialization-opt-in.md):
once the opt-in feature (`include_raw_uri = true`) is enabled, an owned URI
String is part of the feature contract; the marginal allocation is dominated
by the unavoidable per-entry `to_json()` String. Any future finding that
flags "AccessLogEntry per-request URI alloc / RequestInfo should cache
entry_raw_uri / request_uri should return Option<&str>" is closed per this
entry, unless it is proved that (a) profile measurement shows the URI alloc
has measurable share on the request path, or (b) a larger access-log
zero-copy refactor lands that also moves `to_json()` off the request task,
making the URI borrow worth carrying through.
