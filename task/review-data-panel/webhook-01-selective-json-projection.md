# Webhook selective JSON projection

Status: deferred for user decision. No runtime change.
Source baseline: `9472bf826`.

## Evidence and scope

The Body branch of extract_value_from_response validates UTF-8 and deserializes
an entire serde_json::Value before traversing one configured dot path. Unselected
objects, arrays and strings are constructed even when only one scalar is needed.
Selected String ownership now transfers directly out of that tree; the preceding
clone removal does not eliminate unselected subtree construction.

Consider a selective visitor or validated raw-range projection only after measuring
its complexity and benefit against the existing parser. Reusing serde machinery
can reduce construction work, but skipping values is not automatically equivalent
to parsing them as Value. Retaining the current implementation is a valid option.
No prototype, allocation total or throughput improvement is established here.

This is distinct from json-02's DSL validation-only parser and work ledger. That
parser has its own strict duplicate and budget rules; importing its policy would
change webhook behavior. Configured path preparation is also a separate mechanism,
not implicitly authorized by this proposal.

## Required semantics

The current implementation parses the complete document before lookup. It can
reject errors in an unselected branch, malformed trailing input, invalid UTF-8,
invalid string escapes, depth violations and numbers that Value cannot represent.
A visitor that returns on the first matching field or skips unselected number
validation is not equivalent. Preserve the locked serde_json behavior, including
its enabled features, rather than claiming generic JSON syntax is sufficient.

Decoded duplicate object keys overwrite earlier values. Selection must use the
last value, including when a later duplicate changes the branch type or removes
a previously selectable descendant. Do not change this to first-match selection
or strict duplicate rejection as a performance edit.

Each dot segment first attempts Rust usize parsing. Numeric segments select array
indices and do not fall back to numeric object keys after lookup failure. This
includes accepted spellings such as 00 and +0. An integer-overflow parse failure
instead proceeds as a string-key lookup. Empty segments remain string keys; there
is no JSON Pointer escaping or root-selection shorthand.

Selected strings return decoded text, null returns None, and all other selected
values use serde_json serialization. Returning a raw substring changes whitespace,
number spelling and object serialization behavior, so it is not a drop-in result.
Missing paths and invalid documents continue to yield None and let WebhookKeyGet
apply its existing default/skip policy.

## Boundaries and acceptance

Use IMPL with an exact allow-list and independent plan/final review. Keep complete,
bounded outbound response acquisition and client status/body classification before
extraction. Preserve overflow-to-failure and transport cancellation; this does not
allow early response acceptance, another body cache, main-response aggregation,
new limits, logs or metrics. Header, Cookie and BodyText extraction are outside scope.

Differentially compare the existing extractor against any candidate for nested
arrays/objects, repeated decoded keys, type-changing duplicate branches, empty and
numeric-shaped path segments, numeric overflow keys, missing/null/string/scalar/
composite results and escaped text. Include invalid unselected subtrees, trailing
content, recursion boundaries and numeric representation errors. Pin final output
bytes and default/skip outcomes, not just whether a matching field was found.

Measure shallow/wide/deep bodies with early and late selected fields, large
unselected strings, repeated keys, missing paths and composite selections. Include
full-document validation work, parser scratch space, retained response bytes and
output ownership. Smaller constructed trees do not imply zero allocation or a
precise heap bound; keep performance claims specific to measured fixtures.

## Sources and verification limits

- Gateway `link_sys/providers/webhook/resolver.rs`: extract_value_from_response.
- Gateway `plugins/http/webhook_key_get/plugin.rs`: default and skip behavior.
- Gateway `plugins/http/common/outbound/call.rs`: bounded acquisition and classification.
- Locked serde_json 1.0.151 `src/value/de.rs` and `src/de.rs`: Value construction,
  duplicate replacement and complete-input validation.
- `skills/04-review/cpu-memory/review-evidence/webhook-read-then-truncate-16kb-cap.md`.

Fresh unchanged-code verification passed all 11 webhook resolver tests. These
establish baseline behavior only; no selective parser or live transport was tested.
