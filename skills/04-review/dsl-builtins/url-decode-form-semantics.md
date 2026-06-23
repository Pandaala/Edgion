---
name: url-decode-form-semantics
description: Use when reviewing DSL `url_decode` builtin findings; `url_decode(s)` intentionally uses application/x-www-form-urlencoded semantics (`+` → space, not RFC 3986). The name-vs-semantics gap is documented; renaming or splitting into `url_decode` (RFC 3986) + `form_decode` (form) is not accepted.
---

# DSL `url_decode` builtin uses form-encoding semantics intentionally — DESIGN-TRADEOFF

## Conclusion

`BuiltinId::UrlDecode` in `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs:448-480`
decodes both `%XX` AND literal `+` (`+` → space), implementing
`application/x-www-form-urlencoded` semantics rather than RFC 3986 path decoding.
The name-vs-semantics mismatch is **documented** (api-reference §`url_decode` carries
an explicit security warning, plus an in-source doc-comment above the arm) and is
**locked** as a design choice: the form semantics intentionally mirror
`uri_utils::url_decode` (`edgion-resources/src/uri_utils.rs`) so the
DSL, route matching, and consistent-hash arg-name comparison (`backends-F014`) all
agree on the same decoded form. Re-introducing the asymmetry by renaming or
splitting the builtin would create a cross-layer divergence.

Fix suggestions of the form "rename to `form_decode` and introduce a new RFC 3986
`url_decode`", "split into `url_decode` (RFC 3986) + `form_decode` (form)", or
"add a `path_decode` sibling builtin" are **not accepted**.

## Core Rationale

**1. Cross-layer consistency with route matching**

`uri_utils::url_decode` (in `edgion-resources/src/uri_utils.rs`) is the canonical decoder used
by HTTP route matching and is shared with `backends-F014` (consistent-hash arg-name
percent-decoding). It uses form semantics (`+` → space). The DSL `url_decode` was
intentionally written to behave the same so operators writing
`url_decode(req.query("foo"))` get the same byte sequence the routing layer would
produce. A DSL builtin that decoded the same input differently would be a
debuggability hazard.

**2. Operator-facing risk is bounded by documentation, not a rename**

The realistic misuse — `url_decode(req.path())` compared against an ACL containing
a literal `+` — requires the operator to:
- explicitly call `url_decode` on path data (the documented intent is query data),
- compare the result against an allowlist whose literal entries contain `+`,
- treat the result as authoritative for an authz decision.

The api-reference §`url_decode` block now carries a labelled **Semantics** and
**Security warning** paragraph telling operators not to do this; the in-source
doc-comment above `BuiltinId::UrlDecode` mirrors the same warning for future
maintainers. A rename or a new RFC 3986 builtin would not address an existing
script that already calls `url_decode(req.path())` — only the documentation does.

**3. Renaming is a breaking change for a Low-severity issue**

The issue is Severity Low and BadLogic-class. Renaming `url_decode` → `form_decode`
breaks every operator-deployed DSL script that already calls it (a "rename" with
deprecation alias is still a multi-release rollout for a niche risk). Adding a
sibling `path_decode` builtin doubles the surface area of the encoding-related
builtin set and forces a parallel decision at the routing layer
(`uri_utils::url_decode` would need a sibling too, or the layers
diverge again). The cost/benefit ratio does not justify either change at the
current severity.

**4. Industry precedent is split, so the name itself is not authoritative**

JavaScript `decodeURIComponent` does **not** decode `+`. Python `urllib.parse.unquote`
does **not** decode `+`; `urllib.parse.unquote_plus` does. Java
`URLDecoder.decode` **does** decode `+`. Ruby `CGI.unescape` **does** decode `+`.
Go `url.QueryUnescape` does; `url.PathUnescape` does not. There is no industry
consensus that the bare name `url_decode` implies RFC 3986; operators coming
from Java/Ruby/Go-query backgrounds would expect the form behavior, and
operators from JS/Python-unquote backgrounds would expect the RFC 3986 behavior.
The right fix is documentation, not picking one side of the naming split.

## Fix Suggestions Not Accepted

- "Rename `url_decode` to `form_decode`" — breaking change for every deployed
  DSL script; the rename does not protect existing scripts and the api-reference
  warning already covers the misuse case.
- "Add a sibling `path_decode(s)` (RFC 3986) builtin and keep `url_decode`" —
  doubles the encoding-builtin surface for a Low-severity, narrow-trigger issue;
  also forces a parallel decision at the routing layer (`uri_utils`
  shared by route matching and consistent-hash arg-name path) to keep cross-layer
  parity, expanding the change well beyond the DSL.
- "Make `url_decode` round-trip-symmetric with `url_encode`" — `url_encode`
  already encodes `+` as `%2B`, so `url_decode(url_encode(s)) == s` for any
  input that does not itself contain a literal `+`. Making the round-trip
  symmetric for literal-`+` inputs would require dropping form semantics, i.e.
  the same rejected rename above.
- "Emit a runtime warning when `url_decode` is called on a value that looks
  like a path" — the DSL has no value-tagging for "this is a path" (`req.path()`
  returns a `Str` like any other), and Iron rule 3 of
  `skills/03-coding/01-log-safety.md` forbids `tracing::*` on the data-plane
  hot path regardless of level.

## Re-evaluation Triggers

Re-open this decision only if:

- A concrete production incident is reported in which a DSL script's
  `url_decode(req.path())` allowlist bypass was exploited (Severity bumps to
  High/Critical, justifying a breaking rename).
- `uri_utils::url_decode` is itself rewritten to RFC 3986 semantics —
  in that case the DSL must follow for cross-layer consistency, and this rule
  is superseded.
- A separate RFC 3986 path-decoding builtin is requested for a different
  use case (e.g. operator-driven path normalization layer) — that would be a
  feature request handled via the brainstorming workflow, not via this rule.

## Reference Cases

- `tasks/plugins/plugins-http-dsl-F021-urldecode-plus-decodes-as-space.md`
  (F-021, 2026-05-26 — original finding, closed as 📝)
- `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs:448-480` —
  `BuiltinId::UrlDecode` arm with the in-source design-rationale comment
- `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference.md`
  §`url_decode` — operator-facing semantics + security warning
- `skills/04-review/backends/consistent-hash-arg-percent-encoded-name.md`
  — sibling decision locking the same form semantics at the routing layer
- `skills/03-coding/dsl/builtin-error-semantics.md` — `url_decode` row already
  catalogues the "silent fallback" error shape (preserves literal `%XX` on
  invalid hex)
