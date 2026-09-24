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

`edgion-waf/src/seclang/semantic.rs` admits the two operator kinds;
`compiler.rs` creates typed operands; and `transaction.rs` charges work,
evaluates the detector, and stages captures. `edgion-waf/src/injection.rs`
enforces a 4 MiB per-value detector cap and a checked linear detector work
charge accumulated across rule members and phases in one transaction. The
existing per-member operator, chain-branch, and request-body stream work
checks use separate local counters against the same configured limit. The native Rust
scanners in `sqli/` and `xss/` use static data pinned to
`libinjection-go` v0.3.2; provenance, license texts, reference verification,
and the local benchmark are described in `edgion-waf/INJECTION.md`.
SQL word delimiter lookups inspect only the token's truncated prefix;
inspecting every prefix of an unbounded dotted word would make request work
quadratic despite the linear charge.

The HTML tokenizer represents EOF separately from every input byte, including
`0xFF`, when skipping whitespace before attributes and values. SQL Q-string
delimiter validation compares unsigned bytes, admitting delimiters at or above
`0x21` including high bytes. The pinned Go parser searches for a high-byte
delimiter encoded as a UTF-8 rune followed by a quote, then advances only two
bytes from the match start. The native tokenizer preserves that search and
cursor behavior. These boundaries prevent malformed byte input
from hiding subsequent XSS tokens or changing SQLi tokenization relative to
the pinned Go behavior.

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
Third-party notice inclusion in binary distribution remains tracked in
`tasks/todo/waf-third-party-license-distribution.md`.

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

## Resolution references

- `waf-sqli-xss-rule-operators`: added native, bounded SQLi and XSS operators
  with strict compilation and Gateway acceptance on 2026-09-24.
