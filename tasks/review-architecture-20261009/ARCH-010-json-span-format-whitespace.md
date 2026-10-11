---
id: ARCH-010
status: candidate
kind: behavior-defect
priority: P3
primary_partition: PART-243
independent_verification: confirmed
---

# Restrict the shared AI JSON parser to JSON whitespace

## Current behavior and call chain

`ai/protocols/json_span.rs::Parser::ws` uses u8::is_ascii_whitespace.
That predicate includes form feed (0x0c), which JSON does not permit as syntax
whitespace. Vertical tab is excluded by this Rust predicate and is not part of
this finding. The mismatch affects both eager and offset-only parsing modes.

An unescaped form feed outside a string can therefore be skipped between a
colon and value, before a document, or after its closing token. For example,
the SSE bytes shown below contain invalid JSON but follow the current successful
parser path:

```rust
b"data: {\"usage\":\x0c{\"prompt_tokens\":7,\"completion_tokens\":9}}\n\n"
```

SseEventRef::semantic_data preserves the valid UTF-8 form-feed byte.
`openai/canonical_sse.rs::observe_event` calls this parser and then observe_usage
without an outer JSON validation pass. `ai/accounting/usage.rs` consumes the
returned numeric counters. Consequently this malformed document can contribute
reported usage instead of taking the existing parse-failure path.

This is a source-derived reproducer, not an executed test. No quota bypass,
production incident or provider conformance claim is established. Native request
metadata additionally uses serde_json validation; that separate path does not
make this canonical SSE parse strict.

## Format authority and bounded correction

The [Rust predicate documentation](https://doc.rust-lang.org/stable/core/primitive.u8.html#method.is_ascii_whitespace)
includes form feed. [RFC 8259 section 2](https://www.rfc-editor.org/rfc/rfc8259.html#section-2)
defines JSON whitespace as space, horizontal tab, line feed and carriage return.
These primary references were checked directly.

Make the existing shared ws helper recognize only those four bytes, keeping the
format rule with the parser used by both modes. Preserve offsets, node/depth
bounds, eager versus selected string decoding, duplicate-key policies, raw SSE
bytes and downstream parse-failure semantics. Escaped form feed inside a JSON
string remains valid and must continue to decode.

Adding a validation pass at each caller duplicates format ownership and parsing
work. Replacing the bounded offset parser or broadening observation/termination
policy is unnecessary for this correction. Treating general ASCII whitespace
as JSON syntax preserves the current mismatch.

## Future validation and original context

Future focused parser cases should reject form feed before, within and after
documents in both modes; accept all four legal whitespace bytes; and retain
valid escaped form feed within strings. A canonical SSE test should drive the
real decoder and observer and show that the invalid usage envelope contributes
no reported counters. Preserve existing split-record, duplicate-field, node/depth,
semantic fallback and usage tests. Use the required private unit-test layout.

Original native Messages/Responses, monotonic usage and malformed-SSE decisions
and canonical observer tests were inspected. The malformed-SSE event-poison
tolerance is a different mechanism and must remain unchanged. Offset-only
unselected-string validation is outside this single whitespace root. Exact
primary/supporting bindings and independent verification live in STATE.json.
