# Implement the Bounded Incremental XML Processor

## Meta

| Key | Value |
|-----|-------|
| Status | pending |
| Priority | P1 |
| Depends on | `06-json-validation-and-benchmark.md` |

## Goal

Implement the classified XML request-body subset using the structured stream
interface established for JSON, without external entities, network access, a
DOM, or full-body retention.

## Work

- Incrementally tokenize start/end tags, names, attributes, text, CDATA,
  comments, and processing instructions required by the accepted profile.
- Preserve UTF-8 and token state across arbitrary chunk boundaries.
- Maintain bounded element/path and namespace state.
- Reject DTD and external/general entity behavior that could cause expansion,
  file access, or network access.
- Enforce depth, attribute-count, name/value bytes, decoded bytes, entries,
  retained bytes, and parser-work limits.
- Publish XML values through the same transaction structured-value interface.
- Keep unsupported XPath or whole-document semantics in `BoundedSnapshot` or
  reject them according to the compiler contract.

## Tests

- Every-byte chunk splits around tags, attributes, UTF-8, CDATA, comments, and
  closing delimiters.
- Malformed nesting, duplicate attributes, truncated input, DTD/entity input,
  and limit boundaries.
- Differential value/order/verdict tests against the existing bounded complete
  XML processor.
- Constant retained-memory tests across bodies much larger than parser state.

## Exit gate

The supported XML subset is incrementally equivalent to the bounded reference,
rejects dangerous expansion behavior, and retains no complete document.
