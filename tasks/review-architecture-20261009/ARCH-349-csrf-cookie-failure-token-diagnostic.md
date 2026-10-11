---
id: ARCH-349
status: confirmed
kind: behavior-defect
priority: P3
partition: PART-465
analyst: /root/analyze_conversion
baseline: 11aeb2347657135da6388057bbf90a4f239e5c16b822a8132cc94916aed7718b
---
# ARCH-349: CSRF cookie-preservation failures publish the issued token

## Claim and current ownership

`test_upstream_cookies_preserved` in `edgion-tests/src/client/suites/edgion_plugins/csrf/csrf.rs:87–137` collects every Set-Cookie value and checks for `upstream_a`, `upstream_b`, and the CSRF cookie. Its failure branch at lines 125–132 formats the entire cookie vector into `TestResult::failed`.

The actual backend emits two fixed upstream cookies. The Gateway appends a freshly generated, signed CSRF token; both local and Kubernetes fixtures select a Secret-backed key and a 7,200-second lifetime. If cookie preservation regresses while CSRF issuance succeeds, the failed assertion contains that complete token. `TestResult::failed` retains the error unchanged, ConsoleReporter prints it, and optional JSON reporting copies it. Synthetic fixture credentials and the test-only topology limit the consequence; this is not evidence of production credentials or an observed disclosure.

## Bounded improvement and alternative

The assertion already computes the useful evidence: each required cookie's presence and the total count. Report those facts and the operation instead of serializing cookie values. Keep full values local to token extraction and comparison where required, and keep every existing success/failure assertion. The other static CSRF diagnostics that explicitly interpolate raw Set-Cookie strings should use the same value-free result policy, without expanding into a repository-wide logging refactor.

A central reporter sanitizer is the inferior alternative: it receives unstructured Strings after token content has already been copied and must guess which fragments are sensitive. Fix the producing diagnostic; no redaction framework, extra runtime allocation, or Gateway change is needed.

## Contracts and future verification

The graded redaction rules and original log-safety page prohibit complete token output. The credential Debug guard explicitly does not protect direct field formatting. The current dynamic CSRF suites already report status and cookie presence without token/key values.

Preserve all three-cookie checks, get_all multiplicity, signed-token validation, safe/unsafe method behavior, missing-key sealing, rotation/deletion/recovery, and the pre-rotation token guard. The accepted append-cookie decision remains valid; this finding concerns its test's failure output, not the production append owner.

Future focused verification should construct the failure with a distinctive issued-token marker and one missing upstream cookie: it must remain failed, identify the missing observation, and omit the marker from console/JSON-bound error data. Retain complete success and missing-CSRF controls and both runners.

## Limits

Static review of actual sources, fixtures and existing test bodies only. No execution or disclosure incident; priority is low because the demonstrated token belongs to the synthetic test topology.
