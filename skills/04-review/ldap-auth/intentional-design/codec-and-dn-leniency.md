---
name: ldap-auth-codec-and-dn-leniency
description: Use when reviewing edgion-ldap decoder strictness (non-minimal INTEGERs, trailing bytes in SearchResultEntry/PartialAttribute, unknown search op diagnostics), DN admission leniency (`\00` escapes, spaces around `=`), the filter encoder's allocations or pre-encoded fast path, or group-requirement size caps.
knowledge_type: intentional-design
---

# edgion-ldap Codec and DN Leniency — Accepted Design

## Conclusion and scope

The `edgion-ldap` leaf crate (`codec.rs`, `proto.rs`, `filter.rs`, `dn.rs`)
deliberately keeps the leniencies below. Each was reported by the 2026-11-10
four-reviewer source review and judged not worth changing; re-reporting needs
new evidence (a concrete security effect or a real interop failure).

| ID | Leniency | Where |
|---|---|---|
| LDAP-SRC-12 | Non-minimal INTEGER/ENUMERATED (`02 03 00 00 01`) accepted | `codec.rs` `Reader::read_u32` |
| LDAP-SRC-13 | Bytes after `attributes` in a SearchResultEntry, or after `vals` in a PartialAttribute, ignored | `proto.rs` `decode_search_frame` |
| LDAP-SRC-15 | Filter encoder allocates two small `Vec`s per encode | `filter.rs` `encode`/`encoded_len` |
| LDAP-SRC-17 | `\00` escapes accepted in configured DNs | `dn.rs` `parse` |
| LDAP-SRC-22 | Found entry DN validated only as UTF-8 (control characters allowed) | `proto.rs` |
| LDAP-SRC-23 | Unknown search protocolOp reports `expected: SEARCH_RESULT_DONE` | `proto.rs` `decode_search_frame` |
| LDAP-SRC-24 | `attribute` matched by exact name only (no schema aliases) | `proto.rs` |
| LDAP-SRC-26 | Pre-encoded group filter recognized by op shape `[Op::Bytes]` | `filter.rs` |
| LDAP-SRC-27 | `group_requirement` has no cap of its own | `filter.rs` |
| LDAP-SRC-28 | Spaces around `=` and before `#` accepted in DNs | `dn.rs` `parse` |

## Rationale and evidence

- **SRC-12:** a message ID must still equal the outstanding request's ID and
  result codes are only compared, so a non-minimal encoding changes nothing.
  It mirrors the accepted non-minimal *length* leniency for OpenLDAP liblber
  ([bounded response parsing](../regression-guards/bounded-response-parsing.md)).
- **SRC-13:** nothing in the tail is retained; the tail stays bounded by its
  enclosing element and the 64 KiB frame cap.
- **SRC-15:** two small allocations are negligible next to the LDAP round trip
  of the same request.
- **SRC-17:** NUL is a legal escaped character in RFC 4514 values.
- **SRC-22:** the DN is only re-encoded as an LDAP OCTET STRING (user bind,
  group search base) and normalized; it never reaches a log, header or response,
  and `Debug` redacts it (see the bounded-response-parsing guard).
- **SRC-23:** fixed-format diagnostic; the outcome (fail closed) is correct.
- **SRC-24:** documented fail-safe behavior (a mismatched name denies); see
  [search mode guards](../regression-guards/search-mode-identity-and-filter.md).
- **SRC-26:** correct by construction: `SearchFilter::compile` always starts
  with `Op::Open`, so only `group_requirement` produces a single `Op::Bytes`.
- **SRC-27:** resources admission caps each group list at 32 DNs (at most 64
  in total) and each DN's length.
- **SRC-28:** a common real-world spelling that servers accept; normalization
  removes the spaces before comparison.

## Alternatives and limits

- Strict X.690 minimal-integer and trailing-element checks were rejected: no
  security benefit and a real risk of rejecting directories that work today.
- A fixed-size array stack in the filter encoder was rejected: it adds an index
  invariant (a panic risk) to the data path for no measurable gain.
- Rejecting `\00` or spaces at admission was rejected: both are valid input
  that servers accept.
- A library-level cap on group DNs was rejected as duplicate: the only caller
  is bounded by admission.

## Verification and re-evaluation

Evidence is the code reading of 2026-11-10 (spec section 9). Reconsider if the
decoder starts retaining any part of a SearchResultEntry tail, if the found DN
is ever written to a text sink, if group lists lose their admission cap, or if
a directory is shown to emit non-minimal INTEGERs that must be rejected.

## Resolution references

- LDAP-SRC-12, -13, -15, -17 (`\00` part), -22, -23, -24, -26, -27, -28 —
  rejected 2026-11-10; spec
  `docs/superpowers/specs/2026-11-10-edgion-ldap-review-fixes-design.md`
  section 9 (rows with the same numbers).
