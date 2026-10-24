# XSS URL prefix decoding reuse

Status: deferred algorithm/performance decision for user review. No runtime change.
Source baseline: `69cc1b129`.

## Evidence

Gateway WAF execution reaches the leaf detector through the compiled DetectXss
operand in `edgion-waf/src/seclang/transaction.rs`, `injection::detect_xss`, and
XssDetector's five HTML contexts. Detector input/work admission precedes execution.

`xss/detector.rs::is_black_url` trims initial raw bytes <= 32 or >= 127, then probes
the static protocol prefixes DATA, VIEW-SOURCE, VBSCRIPT and JAVA in order.
Each htmlencode_startswith call restarts numeric-entity decoding at the trimmed
input's beginning. Long leading encoded whitespace or zero-padded numeric entities
can therefore be decoded repeatedly. Ordinary mismatches short-circuit; this is
up to four prefix probes, not four complete URL scans or quadratic behavior.

The current path retains no decoded URL and allocates no heap normalization
buffer. Tag/attribute lookup separately uses a 64-byte stack normalization buffer
and static binary searches; those mechanisms are outside this candidate.

## Decision alternatives

- Keep the current independent probes if their simple short-circuit behavior is
  faster or the measured gain does not justify another scanner representation.
- Decode a bounded significant prefix once, then compare the current patterns.
  Bound storage by the actual prefix requirement, while preserving how many raw
  bytes may be consumed to obtain it. Do not materialize an entire decoded URL.
- Advance the existing prefix candidates together through one decoder pass.
  Compare bookkeeping cost on ordinary short mismatches with repeated-decoding
  savings on encoded inputs. Do not introduce a generic automaton or cache merely
  because the patterns are static.

No alternative is selected. This is a possible constant-factor CPU optimization,
not an allocation removal or a detector correctness finding.

## Frozen semantics and acceptance

Preserve the behavior pinned in [INJECTION.md](../../edgion-waf/INJECTION.md),
including protocol ordering and all five HTML contexts. In particular:

- Raw initial trimming and decoded leading-byte handling are different stages.
  The decoder recognizes numeric decimal/hex entities, optional semicolons,
  malformed sequences and overflow through its current consumed-byte rules;
  named entities are not generally decoded here.
- While the first significant decoded character is pending, decoded values <= 32
  are skipped. Thereafter NUL and byte value 10 (LF) are always skipped. Do not
  substitute a generic whitespace predicate based on the helper's comments.
- ASCII uppercasing precedes the current cast to u8. Preserve high decoded values
  and low-byte behavior rather than silently applying Unicode or URL normalization.
- Matches are prefixes: no colon, scheme delimiter or complete URL validation is
  required. Replacing these probes with a URL parser would change the detector.

Prove equivalence against the exact pinned local Go reference and current Rust
implementation, using built-in fixtures plus generated/adversarial byte inputs.
Cover each protocol, mixed case, near misses, early/late matches, empty and partial
prefixes, embedded NUL/LF, high raw/decoded bytes, malformed/overflow entities,
optional semicolons, long zero runs and long encoded leading whitespace.
Preserve the 4 MiB value cap, cumulative detector work charges, local work guards,
negation, chain atomicity and SQLi-only capture behavior. Keep linear scanning and
bounded storage; do not reduce work charges based on an unproven average speedup.

Measure ordinary ASCII, benign URLs, short failing prefixes, encoded and adversarial
inputs across representative sizes and HTML contexts. Include end-to-end SecLang
cost as well as the helper, and retain the current implementation if measurements
do not justify change. No throughput or worst-case CPU improvement is established
by this audit. Freeze the exact cross-crate allow-list and obtain independent IMPL
plan/final review before changing this security-sensitive scanner.

## Sources and verification

- `edgion-waf/src/xss/detector.rs`: is_black_url, htmlencode_startswith, detect.
- `edgion-waf/src/xss/blacklists.rs`: protocol table and html_decode_char_at.
- `edgion-waf/src/injection.rs`: bounded work model and detector entry points.
- `edgion-waf/src/seclang/transaction.rs`: detector admission and operator dispatch.
- [Native injection contract](../../skills/04-review/waf/regression-guards/native-injection-operators-use-bounded-detection.md)

Fresh unchanged-source verification passed 13 focused test functions: three
injection, two XSS tokenizer, three built-in differential/robustness, and five transaction
operator/high-byte/work/capture cases. Two external-checkout comparisons remained
ignored. No full pinned-checkout comparison, prototype, benchmark, full test suite
or live Gateway integration ran for this audit.
