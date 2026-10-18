---
name: query-string-dropped
description: RequestRedirectFilter::build_location now preserves the request query string in the Location header. Reject re-reports claiming the Gateway API RequestRedirect filter drops the query string.
knowledge_type: regression-guards
---

# redirect-FB01 — RequestRedirect filter preserves the query string

**Status:** ✅ fixed · 2026-05-22
**Severity:** Medium (BadLogic) — redirect Location silently dropped the client's query string

## Conclusion

`RequestRedirectFilter::build_location`
(`edgion-gateway/src/plugins/runtime/gateway_api_filters/request_redirect.rs`) now appends the
original request query string (`?<query>`, when non-empty) to the assembled `Location` URL.
`run_request` reads `session.request_query()` alongside `get_path()` and passes it into
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
preserved the query — `URLRewriteFilter::rewritten_uri` (`url_rewrite.rs`, `final_uri = path?query`)
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
- Sibling preservation: `URLRewriteFilter::rewritten_uri` (`url_rewrite.rs`),
  `EdgionHttpRedirectProxy::build_redirect_url_from_parts` (`redirect_http.rs`).
- Same redirect-builder area as [redirect-F001](host-split-ipv6-literal.md)
  (IPv6 Host port-stripping).
- Unit tests `build_location_preserves_query_string` /
  `build_location_without_query_has_no_question_mark` /
  `build_location_appends_query_after_non_default_port` (`request_redirect.rs`); integration
  regression `redirect_only_preserves_query`
  (`edgion-tests/src/client/suites/http_route/filters/filter_conflict.rs`, route
  `filter-conflict.example.com/redirect-only`).

## Borrowed current-query ownership

Both native RequestRedirect and URLRewrite borrow the current raw URI query via
request_query. The real adapter reads the same URI as get_query without its
String copy; neither accessor uses the frozen entry query cache. Keep the query
borrow local to constructing the owned Location or upstream URI before session
mutation or await. Missing and empty queries still omit the question mark;
percent escapes, plus signs, repeated keys and separators are copied verbatim.

Callback tests reject use of the owned getter and check missing, empty and raw
query cases. The real-adapter test current_raw_query_borrows_live_uri_without_copying
checks pointer identity, zero accessor allocations and live URI changes against
the independent frozen cache. This eliminates one nonempty query copy per
transform, not allocations for the final URI/Location or response construction.
