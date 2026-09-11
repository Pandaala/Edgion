# Implement the Incremental JSON Tokenizer

## Meta

| Key | Value |
|-----|-------|
| Status | pending |
| Priority | P1 |
| Depends on | `01-structured-streaming-contract.md` |

## Goal

Implement an in-crate JSON tokenizer that accepts arbitrary byte chunks and
retains only bounded token and parser state. It must not retain the complete
JSON document or depend on an external streaming parser.

## Work

- Add a transport-neutral `Data`/`Complete`/`Abandoned` input contract.
- Incrementally recognize objects, arrays, names, strings, numbers, booleans,
  null, separators, and whitespace.
- Preserve string escape and UTF-8 decoder state across chunk boundaries.
- Validate JSON number grammar without copying the full document.
- Enforce token-byte, decoded-byte, depth, entry, retained-byte, and work
  limits before state growth.
- Reject malformed input, incomplete EOS, invalid UTF-8, invalid escapes, and
  invalid surrogate pairs with typed, payload-free errors.
- Expose exact retained-memory accounting for request metrics and tests.

## Tests

- Split every valid and invalid fixture at every byte boundary.
- Cover escape sequences, UTF-8, surrogate pairs, long numbers, empty values,
  nesting, trailing data, truncated tokens, and abandoned streams.
- Assert retained memory is independent of total input length once configured
  token bounds are fixed.
- Run deterministic chunk-layout property tests against the existing complete
  JSON parser for syntax acceptance.

## Exit gate

The tokenizer produces identical syntax outcomes for whole-input and arbitrary
chunked input, fails within declared bounds, and retains no full document.
