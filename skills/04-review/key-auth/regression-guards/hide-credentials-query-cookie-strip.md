---
name: hide-credentials-query-cookie-strip
description: Use when reviewing key_auth hideCredentials findings that claim it is a no-op for query/cookie sources, or that the doc over-promises credential stripping; documents the implemented query/cookie stripping.
status: fixed
finding_id: fr-auth2-02
closed: 2026-05-30
knowledge_type: regression-guards
---

# `key_auth` `hideCredentials` query/cookie no-op — FIXED (fr-auth2-02)

## Conclusion

`KeyAuth::hide_credentials` now strips the authenticating key from all
request-borne sources: `Header` (remove header), `Ctx` (remove ctx var),
`Query` (rewrite the request URI without the matched parameter), and `Cookie`
(re-serialize the `Cookie` header without the matched cookie). Variants that
`extract_key` already ignores (`RemoteIp`, `Path`, `Method`, …) fall through
`_ => {}`. `KeyGet::Webhook` is gone — the former Webhook no-op arm is not a
live contract. If KeyAuth reads a ctx var written by `WebhookKeyGet`,
`hideCredentials` strips that ctx var (existing `Ctx` arm).

Re-reports of the form "`hideCredentials` silently no-ops for query/cookie /
the doc over-promises" are **not accepted** — the gap was closed in
`edgion-gateway/src/plugins/http/key_auth/plugin.rs`. Verify
`hide_credentials` has explicit `KeyGet::Query` / `KeyGet::Cookie` arms
(`strip_query_param` / `strip_cookie`) before re-reporting.

The two helpers now live in
`edgion-gateway/src/plugins/http/common/auth_common.rs` and are shared with
`jwt_auth`; their doc comments carry the decoded-vs-raw key asymmetry, the
multi-line `Cookie` data-loss hazard, and the `set_request_header`
replace-all-lines assumption that this entry records.

## Core Rationale

**1. It was a silent failure of a declared security control.**

Operators set `hideCredentials: true` to keep the API key out of the upstream
request (and out of upstream access logs / log aggregators). Before the fix,
`Query` and `Cookie` (and the deleted `KeyGet::Webhook` variant) all fell into
a `_ => {}` arm, so the raw `?api_key=...` query and the raw `Cookie` header
were forwarded verbatim. No error, no warning — a silently ineffective
security feature.

**2. Stripping must not override the path-forwarding decision.**

- Query: split the live query (`session.request_query()`) on `&`, drop the pair
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

**3. There is no `KeyGet::Webhook` arm to strip.**

The variant is gone. Re-reports that a Webhook `hide_credentials` arm is
missing are not accepted because the variant does not exist. Webhook-resolved
values reach KeyAuth only as `KeyGet::Ctx` (written by `WebhookKeyGet`); the
existing `Ctx` arm strips that var.

## Fix Suggestions Not Accepted

- "Add a `tracing::warn!` when query/cookie stripping is requested" — data-plane
  hot path; Iron rule forbids it. The fix is to actually strip, not to log.
- "Also strip the `Webhook` source" / "add a missing Webhook arm" — the
  variant is gone; webhook-resolved values are `Ctx` and already stripped.
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
- Code: `edgion-gateway/src/plugins/http/key_auth/plugin.rs` — `hide_credentials`;
  `edgion-gateway/src/plugins/http/common/auth_common.rs` — `strip_query_param`,
  `strip_cookie` (shared with `jwt_auth`); `PluginSession::set_request_query`
  (`runtime/traits/session.rs` + `runtime/session_adapter.rs`).
- Docs: `docs/en/.../edgion-plugins/key-auth.md` &
  `docs/zh-CN/.../edgion-plugins/key-auth.md` "Behavior Details" now enumerate
  which sources are stripped.
- `skills/04-review/key-auth/intentional-design/extract-first-not-shared.md` — sibling key-auth
  decision (the variant filter in `extract_key`).

## Resolution references

- `fr-auth2-02` — 2026-05-30 (fixed; retargeted 2026-08-23).

## Transfer the merged Cookie buffer

The shared Cookie-strip path converts its owned merged String directly into
HeaderValue, then passes that value through the private typed sensitive-header
writer. The public string-based auth writer delegates to the same sensitivity
boundary, so every write still marks the value sensitive before the session
setter. All other auth callers retain their existing borrowed-string interface.
Invalid header values and setter errors remain ignored by Cookie stripping;
replacement/removal ordering and multi-line merging remain unchanged.

The pinned http 1.5.0 owned conversion uses the same validation as from_str and
transfers the buffer through bytes 1.12.1. The plan/conversion matrix checks the
join result has capacity equal to length, valid bytes retain their original
pointer, and conversion allocates zero calls/requested bytes. It also compares
owned/borrowed validation for UTF-8, tabs and internal invalid controls. A typed
writer test preserves pointer identity, sensitivity and setter error propagation.
This removes the merged-cookie payload copy; planning, header-map mutation and
the whole auth callback are not claimed to allocate zero times.
