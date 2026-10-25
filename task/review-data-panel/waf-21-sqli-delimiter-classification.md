# SQLi word and variable delimiter classification

Status: deferred for user decision. No runtime representation selected.
Source baseline: `fbd819740`.

## Evidence

The bounded DetectSqli transaction path reaches SqliTokenizer::parse_word and
parse_var in `edgion-waf/src/sqli/tokenizer.rs`. Both advance through borrowed
input and test each candidate byte with contains on a static delimiter slice.
These literals create no request-owned allocation. Repeated membership checks
are visible in source; the optimized compiler lowering and runtime cost have
not been measured.

The sets are intentionally different: word delimiters have 36 distinct bytes,
variable delimiters 31. NUL, square/curly brackets and 0xA0 occur only in the word
set; backtick occurs only in the variable set. A shared union would change token
boundaries. Dot is not a delimiter in either set. parse_word separately examines
dot/backtick prefixes, bounded by the truncated token length, for keyword lookup.
parse_var preserves one/two leading @ markers in count and delegates initially
quoted/backtick names before scanning an ordinary variable name.

## Decision alternatives

- Retain the current literals and contains calls if optimized code already makes
  their cost negligible for representative traffic.
- Evaluate compile-time bitsets with separate word/variable membership.
- Evaluate a static byte-class table with distinct classification bits. One
  shared owner must not collapse the two semantic sets into one classification.

No alternative is selected. Compare code size, static data and cache effects as
well as scan cost. Cover short ordinary identifiers, long words/variables, early
and late delimiters, high bytes, mixed token streams and reparse-heavy inputs.
Measure optimized end-to-end SecLang execution, not only an isolated membership
loop. Fixed-size delimiter sets already bound the per-byte source operation;
this is not evidence of a quadratic scan or a new complexity bound.

This candidate does not require request caches, lazy initialization or copies of
classification tables into every compiled generation. Keep any selected tables
compile-time and scanner-owned. Simple money/tag ASCII predicates, generated
keyword data and [token-storage representation](waf-14-sqli-token-storage.md) are
separate decisions, not permission to expand this refactor.

## Acceptance before implementation

Freeze the exact delimiter memberships across all 256 byte values and compare
candidate classification against them. Preserve public tokenizer output for
NUL/high bytes, brackets, backticks, dots, empty/end-boundary inputs and quoted
variables. Check token kinds, source positions, complete zero-padded values,
31-byte truncation, full cursors, quote metadata and @ counts, not verdicts alone.
Do not replace the bounded dot/backtick prefix loop with an unbounded prefix
scan; retain keyword callback ordering and all fallback paths.

Preserve dialect/quote reparsing, statistics, folding, fingerprints, detector
input/work limits, transaction negation/capture and chain behavior. Use the pinned
reference and existing parser/folding/verdict fixtures; retain the long dotted
word regression. No new request logging, unsafe indexing, unbounded retention or
second scanner is authorized. Obtain independent IMPL plan and final review for
an exact implementation allow-list before adopting a representation.

## Sources and current verification

- `edgion-waf/src/sqli/tokenizer.rs`: parse_word, parse_var, Token::assign.
- `edgion-waf/src/sqli/mod.rs`: detection contexts and folding consumers.
- `edgion-waf/src/injection.rs`: detector input/work contract.
- [Native injection regression guard](../../skills/04-review/waf/regression-guards/native-injection-operators-use-bounded-detection.md).
- [Pinned reference and fixture provenance](../../edgion-waf/INJECTION.md).

Fresh unchanged-production WAF library tests passed 865 cases with six existing
ignored. Formatting and workspace all-target check/Clippy passed with existing
warnings. This verifies the current baseline, not an alternative classification.
No prototype, benchmark, fresh external-checkout comparison, full-workspace test
suite or live Gateway integration ran; no performance benefit is established.
