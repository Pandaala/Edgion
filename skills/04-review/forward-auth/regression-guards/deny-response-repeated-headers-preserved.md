---
name: deny-response-repeated-headers-preserved
description: Use when reviewing whether ForwardAuth deny responses preserve repeated configured client headers such as WWW-Authenticate and Set-Cookie.
knowledge_type: regression-guards
---

# ForwardAuth Deny Responses Preserve Repeated Headers

## Conclusion

`ForwardAuth::send_error_response` preserves every valid text value of each
allowed operator-selected `decision.clientHeaders` name. Protocol-owned fields
and fields nominated by any received `Connection` line are excluded by the
[response mutation policy](../../plugin-runtime/regression-guards/response-header-mutation-policy.md).
A non-text `Connection` value suppresses all remote header copying while the
bounded denial body is still relayed. The first retained value uses
`ResponseHeader::insert_header` to replace any existing values, and later values
use `append_header` so repeatable fields remain separate and ordered.

Case-equivalent duplicate names in `decision.clientHeaders` select the source
field only once. Duplicate source values are not deduplicated: distinct
`WWW-Authenticate` challenges and `Set-Cookie` field lines carry independent
semantics and must reach the client unchanged rather than being comma-joined.

## Root Cause and Fix

The previous implementation selected each configured name with `HeaderMap::get`
and wrote it with `insert_header`. `get` exposed only one value, so later auth
service response values were discarded before the downstream response was built.

The corrected path iterates `HeaderMap::get_all` in source order, filters values
through the existing `HeaderValue::to_str` boundary, and applies set-first then
append-rest semantics directly to the real Pingora `ResponseHeader`. Failed
insert or append operations add the existing `invalid response header` PluginLog
entry. A failed write does not advance the set/append state; values successfully
written before a later append failure remain present.

## Regression Coverage

`deny_copies_client_headers_and_shortcircuits` exercises the private
`handle_outbound_response` path with `MockPluginSession` and inspects the actual
`ResponseHeader` supplied to `write_response_header`. It verifies ordered
multi-value `WWW-Authenticate`, separate `Set-Cookie` fields, case-insensitive
configured-name deduplication, invalid-text filtering, unselected exclusion,
status and content type, zero-copy body ownership, end-of-stream, and shutdown.

`ForwardAuthTestSuite::test_deny_multivalue_headers` adds real HTTP coverage through
the shared Controller/Gateway topology and the existing auth server's dedicated
`/verify-deny-multivalue` endpoint. Local and K8s ForwardAuth fixtures both register
the same plugin and route. The client compares `get_all` byte sequences for two
challenges and two cookies, including a clearance cookie with a comma-containing
Expires attribute. It checks exact per-name order and count, duplicate and
case-equivalent configured-name selection, unselected-header exclusion, 401,
Content-Type, and exact body bytes without logging authentication or cookie values.
Existing success cases retain upstream identity propagation coverage.

This addition addresses the test gap tracked by
`add-forward-auth-deny-multivalue-header-integration`; the task's original claim
that production still selected only the first value was already superseded by
B0290. No production behavior or configuration schema changes are required.
The integration case does not establish HTTP/2, browser cookie-store behavior,
or hot-reload semantics. The local runner exempts only an explicitly selected
`EdgionPlugins/ForwardAuth` run from macOS loopback aliases: every endpoint in
this suite, including its LB fixture, uses `127.0.0.1`. Full runs still require
the aliases needed by multi-address LB and GeoIP suites.

## Integration Verification (2026-09-11)

- Canonical preparation built all integration binaries and helper executables.
  The first run then stopped at the unrelated macOS alias prerequisite; after
  the scoped selection correction, `run_integration.sh --no-prepare
  -r EdgionPlugins -i ForwardAuth` passed all 26 cases, including
  `deny_multivalue_headers` and existing success-side identity cases. The standard
  report at `tmp/integration_testing/testing_20260911_225726/report.log` is PASS;
  metrics-label, unknown-field, requeue-loop, and cache-residue final gates passed.
- Workspace format, all-target check and Clippy passed; Clippy retains unrelated
  repository warnings. Agent-doc, SSA force, metrics inventory, Pingora source,
  English-only, Bash syntax/portability, and shell-helper checks passed.
- Independent read-only code and knowledge review, including the runner change,
  returned LGTM. Live K8s execution and full-workspace unit tests were not run.
- The separate K8s inventory parity check failed on unrelated dynamic-scenario
  registrations; its own regression test passed. The inventory entries were not
  changed by the ForwardAuth work. The later inventory follow-up was removed at
  the user's request, leaving any future reconciliation to the user. The current
  parity command and exception contract remain documented in
  [Kubernetes integration testing](../../../05-testing/02-k8s-integration-testing.md#10-canonical-inventory-parity).

## Scope and Re-evaluation

This guard covers only non-success ForwardAuth responses and only names selected
by `decision.clientHeaders`. The success-side `decision.upstreamHeaders` path
retains its existing first-value request-header replacement semantics. Re-evaluate
if the decision schema gains explicit set/append operations or the deny path no
longer writes a Pingora `ResponseHeader` directly.

## Resolution References

- B0290 — ForwardAuth deny-response repeated-header preservation.
- add-forward-auth-deny-multivalue-header-integration — real HTTP regression coverage.
- `unify-response-header-mutation-policy` — forwarding exclusions preserve allowed repeated fields; current verification is recorded in the shared policy page.
