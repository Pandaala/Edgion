# Replace per-byte delimiter slice searches in the SQLi tokenizer with static byte tables

Priority: P3   Area: plugin:waf (edgion-waf sqli)

## Problem
`SqliTokenizer::parse_word` and `parse_var` in `edgion-waf/src/sqli/tokenizer.rs`
advance through the input with
`while end_pos < slen && !word_chars.contains(&self.input[end_pos])` (36-byte
literal) and the same loop over `var_chars` (31-byte literal). `[u8]::contains`
performs a search of the whole delimiter slice for every input byte. Words are
the bulk of tokenized text, `@detectSQLi` runs on every inspected argument
(CRS PL1), and dialect/quote reparsing can tokenize the same input up to five
times, so this is the tokenizer's innermost loop.

## Proposed change
Define two compile-time `const` 256-entry membership tables (or `[u64; 4]`
bitsets) in the tokenizer, built from the existing delimiter literals by a
`const fn`, one for word delimiters and one for variable delimiters. Replace the
two `contains` calls with a single indexed lookup. Keep the tables scanner-owned
and static; no per-generation or per-request copies.

## Must preserve
- The two sets stay distinct: NUL, `[`, `]`, `{`, `}` and 0xA0 are word-only;
  backtick is variable-only; dot is in neither. Do not merge into one class.
- Exact token boundaries, 31-byte truncation, cursors, quote metadata, `@`
  counts, the bounded dot/backtick prefix loop in `parse_word`, keyword lookup
  ordering, and the quoted/backtick variable delegation in `parse_var`.
- Fingerprints, folding, verdicts and detector input/work limits unchanged.

## Verification
- Add a unit test that, for all 256 byte values, asserts table membership equals
  `literal.contains(&b)` for both the word and the variable literal.
- `cargo test -p edgion-waf sqli` and the injection fixtures pinned in
  `edgion-waf/INJECTION.md` (including the long dotted word regression).
