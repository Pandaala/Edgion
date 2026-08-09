# EdgionDSL request body value

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-07-28 |
| Status | completed |
| Type | feature |
| Priority | P2 |
| Issue | N/A |

## Goal

Replace the implicitly materialized string returned by `req.body` with an
opaque, request-scoped `Body` value. The value exposes familiar string-like
inspection methods and bounded request-body mutation methods while operating
uniformly on memory- and disk-backed storage.

The implementation must never introduce a second request-body cache, expose a
filesystem handle to the DSL, or buffer the main upstream response.

## Current phase

Implementation, documentation, unit tests, workspace checks, the stock Pingora
runtime tests, two real Controller/Gateway DslBody integration runs, and the
final independent deep review are complete. Scan-budget accounting is
explicitly source-chunk granular, matching the I/O work performed. See
[`02-implementation.md`](02-implementation.md) for the exact verification
record.

## Documents

- [`00-context.md`](00-context.md) — requirement, current behavior, constraints,
  and affected modules
- [`01-design.md`](01-design.md) — solution assessment and code-level design
- `02-implementation.md` — implementation and test record, created after design
  review
- `03-subtasks.md` — implementation ownership, created after design review
- `06-decisions.md` — design and code-review decisions, created after review

## Definition of done

- `req.body` is a first-class opaque value and does not implicitly allocate a
  contiguous body string.
- String and Body share the documented semantics of read-only text methods.
- Every Body operation works against memory and file storage through one
  canonical chunk-scanning path.
- Body mutation preserves untouched source ranges and enforces the physical
  request-body cap transactionally.
- Definite JSON paths can be evaluated without building a complete JSON DOM.
- Body I/O and mutation failures fail closed with a generic client response.
- Unit, VM, storage, stock-feature, and DslBody integration coverage pass.
