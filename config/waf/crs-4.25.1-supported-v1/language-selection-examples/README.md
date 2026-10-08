# Explicit language selection examples

These standalone selection documents are optional policy inputs, not automatic
application detection or a claim that a language tag proves irrelevance. Embed
one document under a named WafPolicy profile's `ruleSelection`, retaining that
profile's original ordered bundle references and pins. Default and mixed-stack
profiles remain unfiltered. Include every downstream interpreter and service in
the operator's technology attestation.

The examples are bound to upstream CRS revision
`3b89d5a05322f448b4b74b9cadc5fb05ac6915ad`, the supported package provenance SHA-256
`ca158245e3a70bd3b1a4029026a09a106dab3715d4562c17520657f49f4e08d7`, and the core bundle
digest `eaccaa84c1126f3cc0a12c4fcdad31b98a4a592b2716f11038774f73279242b7`.
Re-audit after any corpus or selection change. Original source bytes and assets
are retained and strictly admitted even when their consumers are excluded.

| Example | Additional omitted root IDs | Expected executable units |
| --- | --- | --- |
| Java without PHP execution | 933100, 933120, 933130, 933131, 933135, 933140, 933150, 933151, 933152, 933153, 933160, 933161, 933170, 933180, 933190, 933210, 933211 | 550 of 567 |
| PHP without Java classloading/deserialization exposure | 944100, 944110, 944120, 944130, 944200, 944210, 944240, 944250, 944260, 944300 | 557 of 567 |

All omitted units execute in phase 2. Their supported-source actions do not write
persistent collections, change runtime controls, jump, or set environment values.
PHP omissions write PHP-injection and inbound PL scores; Java omissions write RCE
and inbound PL scores. Those score contributions intentionally disappear. The PHP
capture consumers and Java MATCHED_VARS consumers are inside their own complete
chains; selection never removes only a child. Retained units do not acquire new
numeric-capture or previous-MATCHED_VAR dependencies from these omissions.

Keep 901 initialization, untagged 933011–933018 and 944011–944018 paranoia controls,
markers, blocking evaluation and 980 reporting. PHP wrapper/file checks 933110,
933111, 933200 and 933220 remain. PHP-tagged generic SSRF rule 934170 remains.
Java rule 944140 and Log4j rules 944150–944152 remain. Generic SQLi, XSS, protocol,
RCE, shell, PowerShell, JavaScript and database families remain unchanged.

`php-variables.data`, `php-function-names-933150.data` and `java-classes.data` are
still baseline-validated. Counts describe compiled membership across all PLs,
not rules executed on every request and not a throughput measurement.

Acceptance is tracked in `edgion-resources/tests/waf_tag_selection.rs`. Tests compare
exact omissions/order, PL1/PL4 findings and scores, retained generic protections,
clean structured bodies and raw Java serialization bytes. Overlapping retained
rules can still detect an omitted rule's payload; omission does not promise allow.
