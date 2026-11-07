---
name: native-injection-operators-use-bounded-detection
description: Review the native WAF SQLi and XSS operators, their budgets, and transaction behavior.
knowledge_type: regression-guards
---

# Native injection operators use bounded detection and transaction semantics

## Conclusion and scope

The native SecLang compiler accepts argument-free `@detectSQLi` and `@detectXSS`
rules and evaluates the selected, transformed HTTP target value through the
existing transaction path. SQLi detection may put its short fingerprint in
`TX.0` when the rule requests `capture`; XSS detection creates no capture.
Negation and chains retain the transaction's staged commit behavior. This
support does not imply that a full CRS ruleset or any other libinjection API is
supported.

## Rationale and evidence

The `edgion-waf/src/seclang/semantic/` module admits the two operator kinds;
`seclang/compiler/operator_plan.rs` creates typed operands; and the
`seclang/transaction/` module charges work, evaluates the detector, and stages
captures. `edgion-waf/src/injection.rs`
enforces a 4 MiB per-value detector cap and a checked linear detector work
charge accumulated across rule members and phases in one transaction.
Ordinary operator work accumulates per rule unit across chain members and
branches ([chain work accounting](chain-ordinary-work-accumulates-per-unit.md));
chain-branch state bytes and request-body prefix/stream work use separate
counters against the same configured limit. The native Rust
scanners in `sqli/` and `xss/` use static data pinned to
`libinjection-go` v0.3.2; provenance, license texts, reference verification,
and the local benchmark are described in `edgion-waf/INJECTION.md`.
SQL word delimiter lookups inspect only the token's truncated prefix;
inspecting every prefix of an unbounded dotted word would make request work
quadratic despite the linear charge.
`parse_word` and `parse_var` classify bytes through two distinct static
256-entry delimiter tables derived from the pinned delimiter literals, and a
unit test asserts membership equal to those literals for all 256 byte values.

The HTML tokenizer represents EOF separately from every input byte, including
`0xFF`, when skipping whitespace before attributes and values. SQL Q-string
delimiter validation compares unsigned bytes, admitting delimiters at or above
`0x21` including high bytes. The pinned Go parser searches for a high-byte
delimiter encoded as a UTF-8 rune followed by a quote, then advances only two
bytes from the match start. The native tokenizer preserves that search and
cursor behavior. These boundaries prevent malformed byte input
from hiding subsequent XSS tokens or changing SQLi tokenization relative to
the pinned Go behavior.

Every comparison of SQL token text against a keyword literal is
ASCII-case-insensitive, because word merging keeps the input's original case.
The 3-token keyword whitelist in `SqliState::handle_three_token_whitelist`
(`edgion-waf/src/sqli/mod.rs`) treats a middle keyword as benign unless its
first four bytes equal `INTO` ignoring ASCII case, matching C
`cstrcasecmp("INTO", val, 4)` and Go `toUpperCmp`. A byte-exact prefix test
whitelisted `x' into outfile '/var/www/s.php`, `1 into outfile 'x'` and other
lower- or mixed-case MySQL file-write payloads that CRS 942100 must catch
(finding `waf4-01`).

Where the pinned Go release has a defect, the scanners follow libinjection C,
and `edgion-waf/INJECTION.md` lists each such difference. The SQL string
parser decides whether a quote is backslash-escaped by scanning back from that
quote; Go v0.3.2 and v0.3.3 inspect the first occurrence of the remaining
suffix instead, so `x'||'a\'b'` is `s&s` in Go and not SQLi here (Go
false-positive side). The CDATA end search is bounded, so `<![CDATA[]]]` and
`<![CDATA[]x]]`, which panic Go v0.3.2, return no XSS verdict like C and
v0.3.3. The number-then-comment whitelist compares the following byte
unsigned, as Go does (C uses a signed char). Word merging uses the C `< 32`
joined-length bound; it differs from Go's `<= 32` only for a 32-byte merged
phrase, which no keyword table entry can produce.

The Gateway's WAF plugin already supplies request targets and transformations;
the operator does not parse an HTTP request or SQL document independently.
Strict resource compilation and the `WafDynamic` integration scenario exercise
publication, hot reload, and HTTP/1.1 and HTTP/2 decisions.

## Alternatives and limits

The implementation uses pure Rust and the existing multi-regex architecture;
Hyperscan and VectorScan remain a separate investigation. The work charge is a
bounded accounting model, not a CPU instruction or wall-clock limit. The
4 MiB cap can reject a selected value even if a surrounding request limit is
larger. The selected production profile is narrower than full CRS coverage.
Third-party notices and pinned provenance are shipped with container images,
Gateway DEB/RPM packages, and binary artifact directories. Distribution locations
and the byte-comparison check are documented in `edgion-waf/INJECTION.md`.
Edgion's project license does not replace the retained upstream licenses.

## Verification and re-evaluation

Implementation-run evidence on 2026-09-24: `cargo test -p edgion-waf --lib`
passed 368 tests with two external-checkout tests ignored; the WAF production
acceptance script passed the existing nine cases and the new dynamic injection
phase. Pinned-reference fixture and generated-input comparisons are described
in `edgion-waf/INJECTION.md`. These results do not establish complete
equivalence for all byte strings or rulesets. Re-evaluate when the detector,
static tables, operator work policy, capture staging, source pin, or selected
Gateway acceptance profile changes.
After review found the unbounded delimiter scan, the 256 KiB dotted-word
regression passed and the optimized end-to-end benchmark measured 32 KiB and
64 KiB dotted-word medians at approximately 98 and 194 microseconds on the
recorded host. This is scaling evidence for those cases, not a universal
worst-case CPU proof.
The later high-byte and cumulative-work corrections have targeted tokenizer,
public transaction, and multi-rule/phase/chain tests; the historical counts
above predate those corrections.

The 2026-11-07 parity pass added these regression tests, each checked against
libinjection-go v0.3.2 and v0.3.3 oracles built from the module cache:
`sqli::tests::three_token_into_outfile_whitelist_is_case_insensitive` (failed
before the `waf4-01` fix on `x' into outfile '/var/www/s.php`),
`sqli::tests::keyword_table_entries_fit_the_merge_buffer`,
`libinjection_diff::known_oracle_divergences_follow_libinjection_c`,
`injection::tests::cdata_lone_right_brackets_do_not_panic_and_match_c`, and the
CDATA rows of `xss::html5::tests::cdata_search_preserves_overlapping_prefixes_and_short_tails`.
Re-evaluate the divergence list when the source pin moves to a release that
fixes either Go defect. Any new keyword-literal comparison on token text needs
a mixed-case vector.

Notice-distribution verification on 2026-10-12 built the runtime and combined
images using existing Linux binaries and compared their shipped notice bytes.
Representative Gateway DEB/RPM packages built through the current nFPM manifest
also contained matching notices. Directory/archive checks passed and missing or
modified notices were rejected. These packaging checks do not establish fresh
detector behavior, Rocky 9 ELF compatibility, or package lifecycle acceptance;
the existing smoke binaries did not satisfy the Rocky 9 glibc baseline. Release
CI checks actual images before push and extracted Gateway packages before upload.

## Resolution references

- `waf-sqli-xss-rule-operators`: added native, bounded SQLi and XSS operators
  with strict compilation and Gateway acceptance on 2026-09-24.
- `waf4-01` (fixed 2026-11-07): the `INTO OUTFILE`/`INTO DUMPFILE` whitelist
  compared case-sensitively and missed lower- and mixed-case payloads.
- Parity review findings F-S4-2 to F-S4-5 (2026-11-07): Go backslash-escape and
  CDATA defects documented as intentional C-following divergences; misleading
  signed-char comment corrected; keyword length bound guarded by a test.
- `waf-third-party-license-distribution`: retained upstream attribution is now
  included in the supported packaging paths, with artifact-content checks.
