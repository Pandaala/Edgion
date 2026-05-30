---
name: hide-credentials-query-cookie-strip
description: Use when reviewing key_auth hideCredentials findings that claim it is a no-op for query/cookie sources, or that the doc over-promises credential stripping; documents the implemented query/cookie stripping.
status: fixed
finding_id: fr-auth2-02
closed: 2026-05-30
---

# `key_auth` `hideCredentials` query/cookie no-op — FIXED (fr-auth2-02)

## Conclusion

`KeyAuth::hide_credentials` now strips the authenticating key from all
request-borne sources: `Header` (remove header), `Ctx` (remove ctx var),
`Query` (rewrite the request URI without the matched parameter), and `Cookie`
(re-serialize the `Cookie` header without the matched cookie). `Webhook`
deliberately remains a no-op because webhook keys are resolved server-side and
never travel in the forwarded request.

Re-reports of the form "`hideCredentials` silently no-ops for query/cookie /
the doc over-promises" are **not accepted** — the gap was closed in
`src/core/gateway/plugins/http/key_auth/plugin.rs`. Verify
`hide_credentials` has explicit `KeyGet::Query` / `KeyGet::Cookie` arms
(`strip_query_param` / `strip_cookie`) before re-reporting.

## Core Rationale

**1. It was a silent failure of a declared security control.**

Operators set `hideCredentials: true` to keep the API key out of the upstream
request (and out of upstream access logs / log aggregators). Before the fix,
`Query`, `Cookie`, and `Webhook` all fell into a `_ => {}` arm, so the raw
`?api_key=...` query and the raw `Cookie` header were forwarded verbatim. No
error, no warning — a silently ineffective security feature.

**2. Stripping must not override the path-forwarding decision.**

- Query: split the live query (`session.get_query()`) on `&`, drop the pair
  whose percent-decoded key matches `name` (mirrors `get_query_param`'s
  decoded-key extraction), then write only the query back via the dedicated
  `PluginSession::set_request_query` (empty string = remove query). Surviving
  pairs keep their original encoding (no decode/re-encode round-trip).
  - **Why a new `set_request_query` method instead of `set_upstream_uri`:**
    `set_upstream_uri` sets `ctx.uri_finalized = true` and rebuilds from the
    *normalized* path (`ctx.path()`). `finalize_request_uri`
    (`pg_upstream_peer.rs`) short-circuits on `uri_finalized`, so calling
    `set_upstream_uri` from KeyAuth would silently force the normalized path
    onto routes configured for raw-path forwarding (`edgion.io/forward-raw-path`
    / global `forward_normalized_path=false` / external-jump `preserve_raw_path`).
    `set_request_query` mutates only the query and leaves `uri_finalized` false,
    so the upstream-peer stage still makes the raw-vs-normalized decision (it
    re-reads the live, now-stripped query via `ctx.query()`).
- Cookie: collect *every* `Cookie` header line via `session.request_headers()`
  (HTTP/2 may split cookies across multiple fields), drop pairs whose raw
  trimmed key matches `name` (mirrors `get_cookie`'s raw-key, first-`=` match),
  then `set_request_header("Cookie", survivors.join("; "))` — or
  `remove_request_header("Cookie")` if none survive. Reading only the first
  line + `set_request_header` (which is `insert_header` = replace-all) would
  drop unrelated cookies carried on other `Cookie` field lines.

**3. `Webhook` staying a no-op is correct, not an oversight.**

The webhook source is resolved by a remote call; the key never appears in the
forwarded request headers/URI, so there is nothing to strip. Re-reports flagging
the `Webhook` arm as an unfixed no-op are not accepted.

## Fix Suggestions Not Accepted

- "Add a `tracing::warn!` when query/cookie stripping is requested" — data-plane
  hot path; Iron rule forbids it. The fix is to actually strip, not to log.
- "Also strip the `Webhook` source" — webhook keys never reach the upstream;
  nothing to strip.
- "Decode-and-re-encode the surviving query pairs via `form_urlencoded`" — would
  alter operator-supplied encoding of unrelated params; preserving the raw pair
  text is intentional.

## Re-evaluation Triggers

Re-open only if:

- `set_request_query` is removed or changed to finalize the URI — the whole
  point is that it leaves the path-forwarding decision to the upstream-peer
  stage; a re-report claiming "query stripping forces normalized path" is only
  valid if that no-finalize contract regressed.
- `finalize_request_uri` stops re-reading the live query via `ctx.query()` (so
  a pre-stage query mutation would no longer survive to the upstream).
- A re-report claims the cookie strip drops unrelated cookies on HTTP/2
  multi-line `Cookie` requests — that was specifically fixed by iterating
  `request_headers()` and merging survivors; verify before re-reporting.

## Reference Cases

- `fr-auth2-02` (2026-05-30) — Opus-confirmed P3 Security; fixed (code + doc).
- Code: `src/core/gateway/plugins/http/key_auth/plugin.rs` — `hide_credentials`,
  `strip_query_param`, `strip_cookie`; `PluginSession::set_request_query`
  (`runtime/traits/session.rs` + `runtime/session_adapter.rs`).
- Docs: `docs/en/.../edgion-plugins/key-auth.md` &
  `docs/zh-CN/.../edgion-plugins/key-auth.md` "Behavior Details" now enumerate
  which sources are stripped.
- `skills/04-review/key-auth/extract-first-not-shared.md` — sibling key-auth
  decision (the variant filter in `extract_key`).
