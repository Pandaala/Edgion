# Supplemental structured-input original-rule corpus

The 195 original argument-floor units in `units.json` are unchanged extracts from
OWASP CRS 4.25.1, including all 173 XML-target units in that set. The eight `.data`
files are unchanged upstream phrase assets. Rule bodies and assets have SHA-256
pins, original source locations and file hashes. The test also pins the complete
unit manifest. Upstream revision is `3b89d5a05322f448b4b74b9cadc5fb05ac6915ad`.

Original copyright notices:

> Copyright (c) 2006-2020 Trustwave and contributors. All rights reserved.
> Copyright (c) 2021-2026 CRS project. All rights reserved.

Original units/assets are distributed under the included Apache 2.0 `LICENSE`.
Source: <https://github.com/coreruleset/coreruleset/tree/v4.25.1>.

`cases.json` is an Edgion-authored supplemental manifest, not an upstream FTW
replacement. Each upstream-derived case identifies the original file/test/stage,
exact file and canonical stage hashes, original expectation, candidate relocation
and declared context. Canonical stage hashes use Python JSON with sorted keys,
ASCII escapes and compact separators. Authored boundary cases name the unchanged
original predicate or action. Candidate scouting only located suitable vectors;
checked-in expectations are fixed and reviewed, not generated during testing.

The JSON conversion relocates an ASCII candidate from an original query/form,
path, cookie, header or body fragment into a key or value. Percent-decoded query
candidates preserve the original attack token. Body fragment extraction is
explicitly attributed and does not claim byte-identical transport normalization.
Cookie/header/path relocations test the same broad original predicate through a
structured field. The `933200.yaml` document incorrectly labels its rule as
`9331200`, while the original stage expectation names `933200`; supplemental
attribution preserves both IDs and declares the corrected grouping.

The isolated profile uses declared test-only Phase 1 TX setup, original phrase
assets and three preceding observation witnesses. The witnesses count raw
REQUEST_BODY, ARGS and XML visibility; their findings never count as original
rule coverage. Clean URI/query/cookies leave the body as the attack source.
`943110` separately requires an off-domain Referer and a session-name key; its
same-host counterpart proves chain/capture/setvar rollback.

Leaf tests compile explicit Phase 2 deny-403 defaults and use the same generation
for On and DetectionOnly. Production WafConfig uses its existing pass defaults;
the resource test verifies full-body strict admission, exact original IDs,
inspection plans, digest pins and representative pass-default execution. It does
not establish Gateway enforcement or change production defaults.

Every valid structured positive asserts automatic processor selection, successful
parsing and absent raw REQUEST_BODY. One-byte feeds cross token, escape and UTF-8
boundaries. Phase outcomes and selected TX/capture effects match owned publication
and reference evaluation. The manifest has 420 fixed cases and 375 JSON escape
or nested-path variants. JSON coverage classifies all 195 original units:

- 191 original attack rules: positive and clean counterparts.
- `921170`: nolog/pass name counter, including sibling and duplicate-key occurrences.
- `920250`, `944200`: valid-input non-activation and malformed On rejection;
  DetectionOnly records the parse error and preserves the separately asserted
  raw fallback. Literal binary fallback activation is not a structured positive.
- `920650`: form/query `_method` selector does not name canonical `json._method`.

XML supplements the three previously missing matching-media IDs: `933200` and
`944260` receive text/attribute attack and clean pairs; `944200` receives explicit
format-constrained tests because its literal bytes violate UTF-8 and XML NUL rules.
This does not claim new activation proof for the other 170 XML IDs in the original
offline media-stage audit.

The suite is hermetic: normal Cargo tests require no restored external CRS tree
or task-folder input. Original rules, original FTW expectations and production
runtime code are unchanged. Generic clean counterparts verify those inputs only;
they are not a broad false-positive corpus or full CRS compatibility claim.

```bash
cargo test --locked -p edgion-waf --test structured_rules_corpus
cargo test --locked -p edgion-resources --test waf_structured_rule_admission
```
