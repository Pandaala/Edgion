---
name: unsafe-upstream-address-log-centralized
description: `UNSAFE_UPSTREAM_ADDRESS_LOG` is now a single `pub(crate) const` in `gateway/routes/common.rs`, imported by all five SSRF-guard call sites (TCP / TLS terminate / TLS passthrough / UDP / HTTP); the TCP fail-close branch now writes the same literal into `TcpContext.deny_reason` so operator grep for `"unsafe upstream address"` matches across every protocol. Reject re-reports claiming the constant is triplicated or that TCP omits the SSRF string from its access log.
---

# path-norm-nonhttp-routes F-B02 — UNSAFE_UPSTREAM_ADDRESS_LOG centralized + TCP path emits the literal

**Status:** ✅ fixed · 2026-05-26
**Severity:** Low (Redundancy) — five scattered declarations of the same literal + TCP fail-close did not surface the literal into any log field

## Conclusion

The SSRF-guard access-log literal `"unsafe upstream address"` is now declared exactly once as
`pub(crate) const UNSAFE_UPSTREAM_ADDRESS_LOG: &str` in
`src/core/gateway/routes/common.rs` (the L4 shared helper module that already hosts
`is_core_service_backend_ref` and `effective_hostnames`). All five SSRF-guard call sites import
it:

1. `routes/tls/proxy.rs` — `ctx.err_log = Some(UNSAFE_UPSTREAM_ADDRESS_LOG.to_string())`
2. `routes/tls/passthrough_proxy.rs` — same field, same constant
3. `routes/udp/edgion_udp.rs` — written into the UDP access-log err field
4. `routes/tcp/edgion_tcp.rs` — `ctx.deny_reason = Some(UNSAFE_UPSTREAM_ADDRESS_LOG)` on the
   `TcpStatus::UnsafeUpstreamAddress` fail-close branch; serialized into the TCP access-log
   JSON `deny_reason` field by `observe/logs/tcp_log.rs`
5. `backends/mod.rs` (`record_unsafe_upstream_attempt`, HTTP-path SSRF reason builder)
   and `routes/http/proxy_http/pg_upstream_peer.rs` (`record_unsafe_resolved_upstream_attempt`)
   — the `_ =>` default arm of the per-status match uses the imported constant (the sibling
   `"unsafe loopback upstream"` / `"unsafe link-local upstream"` arms remain HTTP-specific
   literals; they appear only at these two HTTP call sites and have no L4 counterpart, so
   centralizing them would have no symmetry win)

Before the fix, the literal was redeclared as a per-file `const` in three L4 modules and
appeared inline in two HTTP modules — modifying any one would have silently de-synced log
scraping for the others. Additionally, the TCP path set `TcpStatus::UnsafeUpstreamAddress`
but never emitted the literal into any log field: the access-log `status` is the enum's
`Debug` repr (`"UnsafeUpstreamAddress"`, PascalCase) and contained no `"unsafe upstream
address"` substring, so operator log-scraping rules grepping for the literal silently missed
SSRF-guard hits on TCP.

The TCP path reuses the pre-existing `TcpContext.deny_reason: Option<&'static str>` field —
no new struct field and no new access-log key were introduced. The field's doc-comment phrase
"fail-close / policy denial" already covered SSRF rejection; assigning a `&'static str`
constant is zero-allocation and carries no client-controlled data.

Re-reports of the form "`UNSAFE_UPSTREAM_ADDRESS_LOG` is declared in three / five places",
"TCP path sets `UnsafeUpstreamAddress` but does not write the literal to a log field", or
"operator grep for the SSRF literal misses TCP" are **not accepted** — verify the current
`routes/common.rs` declaration and the `tcp/edgion_tcp.rs` `deny_reason` assignment before
re-reporting.

## Fix Suggestions Not Accepted

- "Centralize the two HTTP-only siblings (`unsafe loopback upstream`,
  `unsafe link-local upstream`) into `routes/common.rs` too." **Not done — out of scope.**
  Those two literals appear at exactly two HTTP call sites (`backends/mod.rs` and
  `pg_upstream_peer.rs`) and have no L4 counterpart. Centralizing them would give no
  cross-protocol symmetry win and would clutter the shared module with HTTP-specific
  observability strings.
- "Move the constant to a new `gateway/constants.rs`." Not accepted — `routes/common.rs`
  already exists, already serves the L4 SSRF-guard observability theme (sibling
  `is_core_service_backend_ref`), and is reachable from `backends/` after the
  `pub(crate) mod common;` visibility bump. A new top-level constants file would invite a
  catch-all dumping ground.
- "Add a new dedicated `TcpContext` field for SSRF reason instead of reusing `deny_reason`."
  Not accepted — the field's existing doc-comment ("fail-close / policy denial") already
  covers SSRF rejection; adding a parallel field would double the access-log key surface and
  drift TCP's log shape from UDP (which already overloads its err field across plugin-deny
  and SSRF).

## Re-evaluation Triggers

Re-open only if:

- A new SSRF-rejecting protocol path is added and emits the literal via a fresh local
  `const`/inline literal instead of importing `UNSAFE_UPSTREAM_ADDRESS_LOG`.
- The TCP `deny_reason` field is repurposed for a non-fail-close concern, requiring the SSRF
  hit to land in a dedicated field again.

## Reference Cases

- `path-norm-nonhttp-routes-FB02-unsafe-upstream-address-log-constant-triplicated` (review
  finding, `tasks/path/`, fixed 2026-05-26)
- `src/core/gateway/routes/common.rs` — canonical declaration site
- `src/core/gateway/observe/logs/tcp_log.rs` — `deny_reason` is `#[serde(skip_serializing_if
  = "Option::is_none")]` and pre-existing tests cover its serialization
