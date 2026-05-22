---
name: redirect-FB01-query-string-dropped
description: RequestRedirectFilter::build_location now preserves the request query string in the Location header. Reject re-reports claiming the Gateway API RequestRedirect filter drops the query string.
---

# redirect-FB01 — RequestRedirect filter preserves the query string

**Status:** ✅ fixed · 2026-05-22
**Severity:** Medium (BadLogic) — redirect Location silently dropped the client's query string

## Conclusion

`RequestRedirectFilter::build_location`
(`src/core/gateway/plugins/runtime/gateway_api_filters/request_redirect.rs`) now appends the
original request query string (`?<query>`, when non-empty) to the assembled `Location` URL.
`run_request` reads `session.get_query()` alongside `get_path()` and passes it into
`build_location`.

Re-reports of the form "RequestRedirect drops the query string", "`build_location` only emits
`scheme://host:port/path`", or "redirect to `/login` loses `?returnTo=...`" are **not accepted** —
the query is preserved.

## What was wrong (before the fix)

`build_location` built `format!("{}://{}{}{}", scheme, hostname, port_str, path)` where `path`
came from `session.get_path()` (the normalized path **without** query). The query was never
read, so a request to `/login?returnTo=/dashboard` that triggered a `RequestRedirect` filter
received `Location: https://host/login`, silently discarding `returnTo=/dashboard` and
corrupting application state (OAuth return-to, search/pagination params, tracking, etc.).

This was an inconsistency, not just a defect: the two sibling redirect/rewrite paths already
preserved the query — `URLRewriteFilter::rewrite_path` (`url_rewrite.rs`, `final_uri = path?query`)
and the route-level `EdgionHttpRedirectProxy` (`redirect_http.rs`, carries `path_and_query()`).
Only the Gateway API `RequestRedirect` plugin path dropped it.

## Why the fix is correct

The query is preserved **unconditionally** (whenever non-empty), regardless of any path
modifier (`replaceFullPath` / `replacePrefixMatch`), mirroring `url_rewrite.rs` and
`redirect_http.rs`. The Gateway API `HTTPRequestRedirectFilter` has no query modifier, so the
client's query must survive the redirect. The Gateway API conformance redirect-path / -scheme /
-port tests do not send a query, so they neither constrain nor regress against this behavior.

The query is appended to the fully-assembled URL (`...path?query`); `get_path()` never returns
a `?`, so there is no double-`?` risk. `None` and empty-string queries both omit the `?`.

## Fix Suggestions Not Accepted

- "Only preserve the query when no path modifier is configured" (the issue's narrower wording) —
  **not accepted**. That would diverge from the sibling `URLRewriteFilter`, which preserves the
  query even on `ReplaceFullPath`; unconditional preservation is the consistent, least-surprising
  behavior.

## Re-evaluation Triggers

Re-open only if:

- A future Gateway API revision adds an explicit query modifier to `HTTPRequestRedirectFilter`
  (then the unconditional-preserve default would need a configurable override).

## Reference Cases

- redirect-FB01 (review finding, Round B adjacent-scope scan; fixed 2026-05-22)
- Sibling preservation: `URLRewriteFilter::rewrite_path` (`url_rewrite.rs:71-74`),
  `EdgionHttpRedirectProxy::build_redirect_url_from_parts` (`redirect_http.rs`).
- Same redirect-builder area as [redirect-F001](redirect-F001-host-split-ipv6-literal.md)
  (IPv6 Host port-stripping).
- Unit tests `build_location_preserves_query_string` /
  `build_location_without_query_has_no_question_mark` /
  `build_location_appends_query_after_non_default_port` (`request_redirect.rs`); integration
  regression `redirect_only_preserves_query`
  (`examples/code/client/suites/http_route/filters/filter_conflict.rs`, route
  `filter-conflict.example.com/redirect-only`).
