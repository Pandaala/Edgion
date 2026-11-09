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

- Query: delegate to the canonical session structured-removal operation. It
  removes all decoded-equivalent names from the logical current query, preserving
  surviving raw encoding and making subsequent ordinary extraction observe removal. It does not
  finalize path or eagerly write Pingora. Current readers consume the owner;
  final forwarding projects Query after backend/provider edits while preserving
  the selected raw/normalized path and native HTTP/1 or HTTP/2 authority. Failed
  projection must not send the old credential-bearing target. No-match removal
  preserves original syntax and does not render. This is the Query portion of
  auth-01; Cookie survivor-storage evaluation remains independent.
- Cookie: delegate to `PluginSession::remove_request_cookies`, whose canonical
  `request_cookie` planner inspects every field before publication. Exact raw,
  case-sensitive names match; values split at the first equals sign. All selected
  duplicates are removed. A match merges surviving trimmed segments with `; `;
  no match leaves raw fields unchanged. An empty removal-name list performs no parsing.
  Invalid UTF-8 anywhere during a nonempty removal request returns a fixed 400;
  internal construction/publication failures return a fixed 500. Callers propagate
  these through `ErrResponse`, preventing later plugins and proxying.

**3. A failed removal denies (since 2026-11-09).**

`strip_query_param` returns the removal result. A header or query removal
failure (for example a credential parameter on a gRPC-JSON-transcoded route,
whose query is sealed) denies with the invalid-key 401 after the failure delay,
PluginLog `credential removal failed`, and the diagnostic
`AuthHeaderMutationRejected`, instead of forwarding the credential. A cookie
removal failure keeps the 400/500 cookie cleanup response above.

**4. There is no `KeyGet::Webhook` arm to strip.**

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

## Canonical Cookie ownership and measured representation

Pingora's current HeaderMap remains authoritative. `PluginSessionHeaderExt::get_cookie_ref`
and the owned getter use `request_cookie::lookup`; raw readers and outbound
snapshots observe successful edits immediately. There is no pending Cookie render
or production parsing cache. Unused cookies require no parsing or assembly.
Lookup retains HeaderValue text eligibility and stops on an opaque field before
its first valued match; editing accepts UTF-8 fields and rejects opaque bytes
atomically. These deliberately distinct eligibility contracts are tested.

The planner retains borrowed survivor metadata and allocates an exact output only
when a name matches. It transfers the owned String into a sensitive HeaderValue;
conversion tests verify validation parity and allocation/pointer behavior on real
planner output. No-match removal cancels an existing deferred Cookie override when
current fields exist, preventing a prior pending header write from resurrecting
credentials. Absent fields retain deferred intent. An empty removal-name list is now an
explicit no-op, including its deferred intent; a list containing an empty name
is distinct and still parses and removes that name.

Measured cache cold-read cost and two-pass matched-removal regressions rejected
those alternatives. The single-pass Vec is an intentional measured tradeoff, not
an eliminated intermediate allocation. The benchmark lives in
`edgion-gateway/tests/benches/request_cookie.rs`; its helper/HeaderMap sequences
measure core operations, and allocation counters report requested bytes rather
than RSS or allocator overhead.
Whole authentication, transport and network throughput are not established by
these source-helper measurements. Re-evaluate representation only with cold reads,
repeated reads, matched/no-match deletion and complete mutation costs.
